use core::cell::UnsafeCell;

use rt::{
    ControlTag, PermissionPolicyState, RawMessage, SecurityStatus, SecurityTag, ServiceId,
    ServiceImageId, TaskStateCode, rights,
};
use serviceos_abi::{IPC_MAX_HANDLES, IPC_MAX_WORDS};
use serviceos_userspace_runtime as rt;

use crate::{
    control::storage::load_image_from_storage,
    state::{MAX_SERVICE_SLOTS, ServiceSlot},
    util::{find_slot_index, find_slot_index_checked},
};

/// Boot-store location of the manually-activated backup-service image. The
/// Backup page in settings-app reaches the service through a public-channel
/// grant minted here at settings-app launch time (see
/// `append_backup_channel_grant`); the shell drives the same image through
/// its own stored-image launch path (shell-service `commands/backup.rs`).
const BACKUP_PROGRAM_PATH: &str = "services/backup-service/program.img";

pub(super) fn launch_program(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    bootstrap_authority: rt::Handle,
    caller: ServiceId,
    image_id: ServiceImageId,
    startup_words: &[u64],
    startup_handles: &[rt::Handle],
    startup_handle_rights: &[u64],
) -> rt::Result<rt::Handle> {
    let bootstrap = rt::channel_create()?;
    let task_handle = rt::service_spawn(image_id, bootstrap_authority, bootstrap.second)?;
    let task_view = rt::handle_duplicate(
        task_handle,
        rights::READ | rights::DUPLICATE | rights::TRANSFER,
    )?;

    let mut startup = RawMessage::empty(ControlTag::Startup as u32);
    populate_startup_message(
        &mut startup,
        startup_words,
        startup_handles,
        startup_handle_rights,
    )?;
    let mut handle_index = startup.handle_count as usize;
    append_launch_grants(
        slots,
        service_count,
        bootstrap_authority,
        caller,
        image_id,
        &mut startup,
        &mut handle_index,
    )?;
    startup.handle_count = handle_index as u32;

    rt::channel_send(bootstrap.first, &startup)?;
    close_startup_handles(&startup);
    let _ = rt::handle_close(task_handle);
    let _ = rt::handle_close(bootstrap.first);
    Ok(task_view)
}

pub(super) fn launch_program_from_image(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    bootstrap_authority: rt::Handle,
    caller: ServiceId,
    image_handle: rt::Handle,
    startup_words: &[u64],
    startup_handles: &[rt::Handle],
    startup_handle_rights: &[u64],
    syscall_abi_flags: u64,
) -> rt::Result<rt::Handle> {
    let bootstrap = rt::channel_create()?;
    let task_handle = rt::task_spawn_image_with_abi(
        image_handle,
        bootstrap_authority,
        bootstrap.second,
        syscall_abi_flags,
    )?;
    let task_view = rt::handle_duplicate(
        task_handle,
        rights::READ | rights::DUPLICATE | rights::TRANSFER,
    )?;

    let mut startup = RawMessage::empty(ControlTag::Startup as u32);
    populate_startup_message(
        &mut startup,
        startup_words,
        startup_handles,
        startup_handle_rights,
    )?;
    let mut handle_index = startup.handle_count as usize;
    append_dynamic_launch_grants(
        slots,
        service_count,
        caller,
        &mut startup,
        &mut handle_index,
    )?;
    startup.handle_count = handle_index as u32;

    rt::channel_send(bootstrap.first, &startup)?;
    close_startup_handles(&startup);
    let _ = rt::handle_close(task_handle);
    let _ = rt::handle_close(bootstrap.first);
    Ok(task_view)
}

pub(super) fn launch_is_authorized(caller: ServiceId, image_id: ServiceImageId) -> bool {
    match caller {
        ServiceId::Shell | ServiceId::Terminal => image_id == ServiceImageId::SysinfoTool,
        ServiceId::Runtime => image_id == ServiceImageId::PosixHostTool,
        ServiceId::Developer => image_id == ServiceImageId::CrossBuilderTool,
        ServiceId::DesktopShell => matches!(
            image_id,
            ServiceImageId::SettingsApp
                | ServiceImageId::FilesApp
                | ServiceImageId::MonitorApp
                | ServiceImageId::TerminalApp
                | ServiceImageId::SoftwareCenterApp
                | ServiceImageId::MediaApp
        ),
        _ => false,
    }
}

pub(super) fn launch_image_is_authorized(caller: ServiceId) -> bool {
    matches!(
        caller,
        ServiceId::Shell | ServiceId::Runtime | ServiceId::Developer | ServiceId::SetupWizard
    )
}

pub(super) fn launch_policy_allows(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    image_id: ServiceImageId,
) -> bool {
    let Some(index) = find_slot_index_checked(slots, service_count, ServiceId::Security) else {
        return true;
    };
    let security = &slots[index];
    if security.phase != crate::state::ServicePhase::Ready
        || security.public_handle == rt::INVALID_HANDLE
    {
        return true;
    }

    let Ok(reply) = rt::channel_create() else {
        return true;
    };
    let mut request = RawMessage::empty(SecurityTag::PolicyInfoRequest as u32);
    request.word_count = 1;
    request.words[0] = image_id as u32 as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    if rt::channel_send(security.public_handle, &request).is_err() {
        let _ = rt::handle_close(reply.first);
        let _ = rt::handle_close(reply.second);
        return true;
    }
    let _ = rt::handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    let result = match rt::channel_receive_blocking(reply.first, &mut response) {
        Ok(())
            if response.tag == SecurityTag::PolicyInfoReply as u32
                && response.word_count >= 4
                && response.words[0] == SecurityStatus::Ok as u32 as u64 =>
        {
            response.words[3] != PermissionPolicyState::Blocked as u32 as u64
        }
        _ => true,
    };
    let _ = rt::handle_close(reply.first);
    result
}

fn populate_startup_message(
    startup: &mut RawMessage,
    startup_words: &[u64],
    startup_handles: &[rt::Handle],
    startup_handle_rights: &[u64],
) -> rt::Result<()> {
    if startup_words.len() > IPC_MAX_WORDS {
        return Err(rt::Error::BufferTooSmall);
    }
    startup.word_count = startup_words.len() as u32;
    for (index, word) in startup_words.iter().copied().enumerate() {
        startup.words[index] = word;
    }

    let mut handle_index = 0usize;
    for (index, handle) in startup_handles.iter().copied().enumerate() {
        if handle_index >= IPC_MAX_HANDLES {
            return Err(rt::Error::BufferTooSmall);
        }
        startup.handles[handle_index] = handle;
        startup.handle_rights[handle_index] =
            startup_handle_rights.get(index).copied().unwrap_or(0);
        handle_index += 1;
    }
    startup.handle_count = handle_index as u32;
    Ok(())
}

fn close_startup_handles(startup: &RawMessage) {
    for handle in startup.handles[..startup.handle_count as usize]
        .iter()
        .copied()
    {
        let _ = rt::handle_close(handle);
    }
}

fn append_launch_grants(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    bootstrap_authority: rt::Handle,
    caller: ServiceId,
    image_id: ServiceImageId,
    startup: &mut RawMessage,
    handle_index: &mut usize,
) -> rt::Result<()> {
    if caller != ServiceId::DesktopShell {
        return Ok(());
    }

    match image_id {
        ServiceImageId::SettingsApp => {
            append_service_launch_handle(
                slots,
                service_count,
                ServiceId::Config,
                rights::SEND | rights::TRANSFER,
                startup,
                handle_index,
            )?;
            append_service_launch_handle(
                slots,
                service_count,
                ServiceId::Network,
                rights::SEND | rights::TRANSFER,
                startup,
                handle_index,
            )?;
            append_service_launch_handle(
                slots,
                service_count,
                ServiceId::Audio,
                rights::SEND | rights::TRANSFER,
                startup,
                handle_index,
            )?;
            append_service_launch_handle(
                slots,
                service_count,
                ServiceId::Security,
                rights::SEND | rights::TRANSFER,
                startup,
                handle_index,
            )?;
            // The Backup page reaches backup-service through the channel
            // granted here (handles[7], present only when the Runtime grant
            // above landed at handles[6] — the gating keeps the positional
            // contract deterministic). Absent grant => the page degrades to
            // its manual-activation explainer.
            let runtime_granted = append_service_launch_handle(
                slots,
                service_count,
                ServiceId::Runtime,
                rights::SEND | rights::TRANSFER,
                startup,
                handle_index,
            )
            .is_ok();
            if runtime_granted {
                let _ = append_backup_channel_grant(
                    slots,
                    service_count,
                    bootstrap_authority,
                    startup,
                    handle_index,
                );
            }
        }
        ServiceImageId::FilesApp => {
            append_service_launch_handle(
                slots,
                service_count,
                ServiceId::Storage,
                rights::SEND | rights::TRANSFER,
                startup,
                handle_index,
            )?;
        }
        ServiceImageId::MediaApp => {
            append_service_launch_handle(
                slots,
                service_count,
                ServiceId::Storage,
                rights::SEND | rights::TRANSFER,
                startup,
                handle_index,
            )?;
        }
        ServiceImageId::MonitorApp => {
            append_service_launch_handle(
                slots,
                service_count,
                ServiceId::Status,
                rights::SEND | rights::TRANSFER,
                startup,
                handle_index,
            )?;
            append_service_launch_handle(
                slots,
                service_count,
                ServiceId::Network,
                rights::SEND | rights::TRANSFER,
                startup,
                handle_index,
            )?;
        }
        ServiceImageId::TerminalApp => {
            append_service_launch_handle(
                slots,
                service_count,
                ServiceId::Terminal,
                rights::SEND | rights::TRANSFER,
                startup,
                handle_index,
            )?;
            append_service_launch_handle(
                slots,
                service_count,
                ServiceId::Clipboard,
                rights::SEND | rights::TRANSFER,
                startup,
                handle_index,
            )?;
        }
        ServiceImageId::SoftwareCenterApp => {
            append_service_launch_handle(
                slots,
                service_count,
                ServiceId::Package,
                rights::SEND | rights::TRANSFER,
                startup,
                handle_index,
            )?;
            // Live package-operation progress: the software center streams
            // package-service's per-phase progress records (the same stream
            // `pkg --verbose` follows). Send-only, appended after the
            // positional handles; absent when log-service is not up, in
            // which case the app degrades to final-reply-only rendering.
            let _ = append_service_launch_handle(
                slots,
                service_count,
                ServiceId::Log,
                rights::SEND | rights::TRANSFER,
                startup,
                handle_index,
            );
        }
        _ => {}
    }

    Ok(())
}

struct BackupGrantCache {
    task_view: rt::Handle,
    public: rt::Handle,
}

struct BackupGrantCacheSlot(UnsafeCell<BackupGrantCache>);

// SAFETY: the manager's supervision loop is strictly single-threaded (same
// shape as shell-service's channel caches).
unsafe impl Sync for BackupGrantCacheSlot {}

static BACKUP_GRANT_CACHE: BackupGrantCacheSlot =
    BackupGrantCacheSlot(UnsafeCell::new(BackupGrantCache {
        task_view: rt::INVALID_HANDLE,
        public: rt::INVALID_HANDLE,
    }));

fn backup_grant_cache() -> &'static mut BackupGrantCache {
    // SAFETY: single-threaded manager loop.
    unsafe { &mut *BACKUP_GRANT_CACHE.0.get() }
}

/// Grant settings-app the backup-service public channel (handles[7]).
///
/// Mirrors the shell's stored-image launch handshake (storage grant first,
/// announcer second): the image is loaded from the boot store, spawned, and
/// its announce carrying the public send-half is awaited here. The channel
/// is cached across settings-app launches and re-validated through the task
/// view, so a crashed instance is respawned on the next launch. Any failure
/// leaves the grant absent — the page renders its manual-activation
/// explainer — and never fails the settings-app launch itself.
fn append_backup_channel_grant(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    bootstrap_authority: rt::Handle,
    startup: &mut RawMessage,
    handle_index: &mut usize,
) -> rt::Result<()> {
    if *handle_index >= IPC_MAX_HANDLES {
        return Err(rt::Error::BufferTooSmall);
    }

    let public = backup_grant_cache().public;
    let task_view = backup_grant_cache().task_view;
    if public != rt::INVALID_HANDLE {
        match rt::task_status(task_view) {
            Ok(status)
                if !matches!(status.state, TaskStateCode::Exited | TaskStateCode::Faulted) =>
            {
                // Cached instance still running: reuse its channel.
                let granted = rt::handle_duplicate(public, rights::SEND | rights::TRANSFER)?;
                startup.handles[*handle_index] = granted;
                startup.handle_rights[*handle_index] = rights::SEND;
                *handle_index += 1;
                return Ok(());
            }
            _ => {
                // Stale cache entry: drop the dead handles and respawn.
                let _ = rt::handle_close(public);
                let _ = rt::handle_close(task_view);
            }
        }
    }

    let image_handle = load_image_from_storage(slots, service_count, BACKUP_PROGRAM_PATH)?;
    let announcer = match rt::channel_create() {
        Ok(pair) => pair,
        Err(error) => {
            let _ = rt::handle_close(image_handle);
            return Err(error);
        }
    };
    // Storage grant first (the service exits without it), announcer second:
    // backup-service's positional startup contract, same as the shell path.
    let storage_index = find_slot_index(slots, service_count, ServiceId::Storage)?;
    let storage_grant = rt::handle_duplicate(
        slots[storage_index].public_handle,
        rights::SEND | rights::TRANSFER | rights::DUPLICATE,
    );
    let storage_grant = match storage_grant {
        Ok(handle) => handle,
        Err(error) => {
            let _ = rt::handle_close(announcer.first);
            let _ = rt::handle_close(announcer.second);
            let _ = rt::handle_close(image_handle);
            return Err(error);
        }
    };

    let startup_handles = [storage_grant, announcer.second];
    let startup_rights = [
        rights::SEND | rights::TRANSFER,
        rights::SEND | rights::TRANSFER,
    ];
    let spawned = launch_program_from_image(
        slots,
        service_count,
        bootstrap_authority,
        ServiceId::DesktopShell,
        image_handle,
        &[],
        &startup_handles,
        &startup_rights,
        serviceos_abi::linux_abi::spawn_abi::NATIVE,
    );
    let _ = rt::handle_close(storage_grant);
    let _ = rt::handle_close(announcer.second);
    let _ = rt::handle_close(image_handle);
    let task_view = match spawned {
        Ok(handle) => handle,
        Err(error) => {
            let _ = rt::handle_close(announcer.first);
            return Err(error);
        }
    };

    // Await the child's announce carrying its public send-half.
    const ANNOUNCE_WAIT_ITERATIONS: usize = 5000;
    let mut announced = rt::INVALID_HANDLE;
    for _ in 0..ANNOUNCE_WAIT_ITERATIONS {
        let mut message = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(announcer.first, &mut message) {
            Ok(()) => {
                if message.handle_count >= 1 {
                    announced = message.handles[0];
                }
                break;
            }
            Err(rt::Error::QueueEmpty) => {
                if rt::yield_current().is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let _ = rt::handle_close(announcer.first);
    if announced == rt::INVALID_HANDLE {
        let _ = rt::handle_close(task_view);
        return Err(rt::Error::Busy);
    }

    let granted = rt::handle_duplicate(announced, rights::SEND | rights::TRANSFER);
    match granted {
        Ok(handle) => {
            let cache = backup_grant_cache();
            cache.task_view = task_view;
            cache.public = announced;
            startup.handles[*handle_index] = handle;
            startup.handle_rights[*handle_index] = rights::SEND;
            *handle_index += 1;
            Ok(())
        }
        Err(error) => {
            let _ = rt::handle_close(announced);
            let _ = rt::handle_close(task_view);
            Err(error)
        }
    }
}

fn append_dynamic_launch_grants(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    caller: ServiceId,
    startup: &mut RawMessage,
    handle_index: &mut usize,
) -> rt::Result<()> {
    match caller {
        ServiceId::Shell => append_service_launch_handle(
            slots,
            service_count,
            ServiceId::Console,
            rights::SEND | rights::TRANSFER,
            startup,
            handle_index,
        ),
        // Setup-wizard launches (account-service during first-boot setup)
        // receive the storage channel so the launched image can persist its
        // own state; handles[0] stays the storage convention.
        ServiceId::SetupWizard => append_service_launch_handle(
            slots,
            service_count,
            ServiceId::Storage,
            rights::SEND | rights::TRANSFER,
            startup,
            handle_index,
        ),
        ServiceId::Runtime => append_service_launch_handle(
            slots,
            service_count,
            ServiceId::Runtime,
            rights::SEND | rights::TRANSFER,
            startup,
            handle_index,
        ),
        ServiceId::Developer => append_service_launch_handle(
            slots,
            service_count,
            ServiceId::Developer,
            rights::SEND | rights::TRANSFER,
            startup,
            handle_index,
        ),
        _ => Ok(()),
    }
}

fn append_service_launch_handle(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    service_id: ServiceId,
    rights_mask: u64,
    startup: &mut RawMessage,
    handle_index: &mut usize,
) -> rt::Result<()> {
    if *handle_index >= IPC_MAX_HANDLES {
        return Err(rt::Error::BufferTooSmall);
    }
    let index = find_slot_index(slots, service_count, service_id)?;
    let transferred =
        rt::handle_duplicate(slots[index].public_handle, rights_mask | rights::DUPLICATE)?;
    startup.handles[*handle_index] = transferred;
    startup.handle_rights[*handle_index] = rights_mask & !rights::TRANSFER;
    *handle_index += 1;
    Ok(())
}
