use rt::{
    ControlTag, PermissionPolicyState, RawMessage, SecurityStatus, SecurityTag, ServiceId,
    ServiceImageId, rights,
};
use serviceos_abi::{IPC_MAX_HANDLES, IPC_MAX_WORDS};
use serviceos_userspace_runtime as rt;

use crate::{
    state::{MAX_SERVICE_SLOTS, ServiceSlot},
    util::{find_slot_index, find_slot_index_checked},
};

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
) -> rt::Result<rt::Handle> {
    let bootstrap = rt::channel_create()?;
    let task_handle = rt::task_spawn_image(image_handle, bootstrap_authority, bootstrap.second)?;
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
            let _ = append_service_launch_handle(
                slots,
                service_count,
                ServiceId::Runtime,
                rights::SEND | rights::TRANSFER,
                startup,
                handle_index,
            );
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
        }
        _ => {}
    }

    Ok(())
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
