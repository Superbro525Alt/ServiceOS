#![no_std]
#![no_main]

use serviceos_bundle::{BOOT_STORE_PATH_MAX, RestartPolicy, ServiceManifest, parse_manifest};
use serviceos_userspace_runtime as rt;
use rt::{
    ControlTag, LifecycleEvent, LogDomain, LogEvent, LogSeverity, LookupStatus, ManagerAction,
    ManagerServicePhase, ManagerStatus, ManagerTag, RawMessage, ServiceId, ServiceImageId,
    TaskStateCode, IPC_MAX_HANDLES, IPC_MAX_WORDS, rights,
};

const MAX_SERVICE_SLOTS: usize = 10;
const MAX_INDEX_BYTES: usize = 512;
const MAX_MANIFEST_BYTES: usize = 512;

#[derive(Clone, Copy)]
struct ServiceSlot {
    manifest: ServiceManifest,
    task_handle: rt::Handle,
    control_handle: rt::Handle,
    public_handle: rt::Handle,
    attempts: u32,
    phase: ServicePhase,
    last_exit_code: u64,
    restart_requested: bool,
    occupied: bool,
    dynamic: bool,
}

impl ServiceSlot {
    const fn empty() -> Self {
        Self {
            manifest: ServiceManifest::empty(),
            task_handle: rt::INVALID_HANDLE,
            control_handle: rt::INVALID_HANDLE,
            public_handle: rt::INVALID_HANDLE,
            attempts: 0,
            phase: ServicePhase::Dormant,
            last_exit_code: 0,
            restart_requested: false,
            occupied: false,
            dynamic: false,
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ServicePhase {
    Dormant,
    Starting,
    Ready,
    Exited,
}

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf601;
    }
    if startup.tag != ControlTag::Startup as u32 || startup.handle_count < 2 || startup.word_count < 1
    {
        return 0xf602;
    }

    let bootstore_handle = startup.handles[0];
    let bootstrap_authority = startup.handles[1];
    let bootstore_len = startup.words[0] as usize;

    fallback_log("bootstrap started");

    let mut slots = [ServiceSlot::empty(); MAX_SERVICE_SLOTS];
    slots[0].manifest = storage_manifest();
    slots[0].occupied = true;
    let mut service_count = 1usize;

    if start_service(
        &mut slots,
        service_count,
        0,
        bootstrap_authority,
        Some((bootstore_handle, bootstore_len)),
    )
    .is_err()
    {
        return 0xf603;
    }
    if wait_until_ready(
        &mut slots,
        &mut service_count,
        bootstrap_authority,
        ServiceId::Storage,
    )
    .is_err()
    {
        return 0xf604;
    }

    if load_base_service_graph(&mut slots, &mut service_count).is_err() {
        return 0xf605;
    }
    if activate_base_service_graph(&mut slots, &mut service_count, bootstrap_authority).is_err() {
        return 0xf606;
    }

    let _ = emit_manager_event(
        &slots,
        service_count,
        LogSeverity::Info,
        LogEvent::ServiceReady,
        ServiceId::RootManager,
        0,
    );

    supervision_loop(
        &mut slots,
        &mut service_count,
        bootstrap_authority,
        (bootstore_handle, bootstore_len),
    )
}

fn storage_manifest() -> ServiceManifest {
    let mut manifest = ServiceManifest::empty();
    manifest.service_id = ServiceId::Storage;
    manifest.image_id = ServiceImageId::StorageService;
    manifest.restart = RestartPolicy::OnFailure { max_restarts: 1 };
    manifest
}

fn load_base_service_graph(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: &mut usize,
) -> rt::Result<()> {
    let storage_index = find_slot_index(slots, *service_count, ServiceId::Storage)?;
    let storage_handle = slots[storage_index].public_handle;
    let (index_handle, index_len) = rt::storage_open(storage_handle, "services/index.txt")?;
    let mut index_buffer = [0u8; MAX_INDEX_BYTES];
    let requested = index_len.min(index_buffer.len());
    let loaded = rt::storage_read_all(index_handle, &mut index_buffer, requested)?;
    let _ = rt::storage_blob_close(index_handle);

    let index_text =
        core::str::from_utf8(&index_buffer[..loaded]).map_err(|_| rt::Error::InvalidArgument)?;
    for line in index_text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let manifest = load_manifest_from_storage(slots, *service_count, line)?;
        let slot_index = allocate_slot(slots, service_count)?;
        slots[slot_index] = ServiceSlot {
            manifest,
            occupied: true,
            dynamic: false,
            ..ServiceSlot::empty()
        };
        let _ = emit_manager_event(
            slots,
            *service_count,
            LogSeverity::Info,
            LogEvent::ManifestLoaded,
            manifest.service_id,
            loaded as u64,
        );
    }

    let _ = fallback_logf(format_args!(
        "loaded {} service manifests",
        occupied_service_count(slots, *service_count).saturating_sub(1)
    ));
    Ok(())
}

fn activate_base_service_graph(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: &mut usize,
    bootstrap_authority: rt::Handle,
) -> rt::Result<()> {
    loop {
        let total = occupied_service_count(slots, *service_count);
        let ready = ready_service_count(slots, *service_count);
        if ready == total {
            return Ok(());
        }

        let mut progress = false;
        for index in 0..*service_count {
            if !slots[index].occupied || slots[index].phase != ServicePhase::Dormant {
                continue;
            }
            if dependencies_ready(slots, *service_count, index) {
                let _ = fallback_logf(format_args!(
                    "activating {}",
                    service_name(slots[index].manifest.service_id)
                ));
                start_service(slots, *service_count, index, bootstrap_authority, None)?;
                wait_until_ready(
                    slots,
                    service_count,
                    bootstrap_authority,
                    slots[index].manifest.service_id,
                )?;
                progress = true;
            }
        }

        if !progress {
            return Err(rt::Error::Busy);
        }
    }
}

fn start_service(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    index: usize,
    bootstrap_authority: rt::Handle,
    bootstrap_resource: Option<(rt::Handle, usize)>,
) -> rt::Result<()> {
    let manifest = slots[index].manifest;
    if bootstrap_resource.is_none() && !dependencies_ready(slots, service_count, index) {
        return Err(rt::Error::Busy);
    }

    close_slot_handles(&mut slots[index]);

    let channels = rt::channel_create()?;
    let task_handle = rt::service_spawn(manifest.image_id, bootstrap_authority, channels.second)?;
    slots[index].task_handle = task_handle;
    slots[index].control_handle = channels.first;
    slots[index].attempts = slots[index].attempts.saturating_add(1);
    slots[index].phase = ServicePhase::Starting;
    slots[index].last_exit_code = 0;
    slots[index].restart_requested = false;

    let mut startup = RawMessage::empty(ControlTag::Startup as u32);
    startup.word_count = 5;
    startup.words[0] = manifest.service_id as u32 as u64;
    startup.words[1] = slots[index].attempts as u64;
    startup.words[2] = manifest.grant_count as u64;
    startup.words[3] = manifest.resource_count as u64;
    startup.words[4] = bootstrap_resource.map(|(_, len)| len as u64).unwrap_or(0);

    let mut handle_index = 0usize;
    if let Some((handle, _)) = bootstrap_resource {
        startup.handle_count = 1;
        startup.handles[0] =
            rt::handle_duplicate(handle, rights::READ | rights::DUPLICATE | rights::TRANSFER)?;
        startup.handle_rights[0] = rights::READ | rights::DUPLICATE | rights::TRANSFER;
        handle_index = 1;
    }

    if handle_index + manifest.grant_count + manifest.resource_count > IPC_MAX_HANDLES {
        return Err(rt::Error::BufferTooSmall);
    }

    for grant in manifest.grants[..manifest.grant_count].iter().copied() {
        let source_index = find_slot_index(slots, service_count, grant.target)?;
        let source = slots[source_index].public_handle;
        let transferred =
            rt::handle_duplicate(source, grant.rights | rights::DUPLICATE | rights::TRANSFER)?;
        startup.handles[handle_index] = transferred;
        startup.handle_rights[handle_index] = grant.rights;
        startup.handle_count += 1;
        handle_index += 1;
    }

    if manifest.resource_count > 0 {
        let storage_index = find_slot_index(slots, service_count, ServiceId::Storage)?;
        let storage_handle = slots[storage_index].public_handle;
        for resource in manifest.resources[..manifest.resource_count].iter() {
            let (resource_handle, resource_len) = rt::storage_open(
                storage_handle,
                resource.as_str().map_err(|_| rt::Error::InvalidArgument)?,
            )?;
            startup.handles[handle_index] = resource_handle;
            startup.handle_rights[handle_index] = rights::SEND | rights::RECEIVE;
            startup.words[4] = resource_len as u64;
            startup.handle_count += 1;
            handle_index += 1;
            let _ = emit_manager_event(
                slots,
                service_count,
                LogSeverity::Info,
                LogEvent::ResourceOpened,
                manifest.service_id,
                resource_len as u64,
            );
        }
    }

    rt::channel_send(slots[index].control_handle, &startup)?;
    for handle in startup.handles[..startup.handle_count as usize]
        .iter()
        .copied()
    {
        let _ = rt::handle_close(handle);
    }

    let _ = emit_manager_event(
        slots,
        service_count,
        LogSeverity::Info,
        LogEvent::ServiceStarted,
        manifest.service_id,
        slots[index].attempts as u64,
    );
    Ok(())
}

fn wait_until_ready(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: &mut usize,
    bootstrap_authority: rt::Handle,
    service_id: ServiceId,
) -> rt::Result<()> {
    loop {
        pump_control_channels(slots, service_count, bootstrap_authority)?;
        let slot_index = find_slot_index(slots, *service_count, service_id)?;
        let slot = &slots[slot_index];
        if slot.phase == ServicePhase::Ready {
            return Ok(());
        }
        let status = rt::task_status(slot.task_handle)?;
        if status.state == TaskStateCode::Exited {
            let _ = fallback_manager_event(
                LogSeverity::Error,
                LogEvent::ServiceFailed,
                service_id,
                status.exit_code,
            );
            return Err(rt::Error::Busy);
        }
        rt::yield_current()?;
    }
}

fn supervision_loop(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: &mut usize,
    bootstrap_authority: rt::Handle,
    bootstrap_resource: (rt::Handle, usize),
) -> u64 {
    loop {
        if pump_control_channels(slots, service_count, bootstrap_authority).is_err() {
            return 0xf610;
        }

        for index in 0..*service_count {
            if !slots[index].occupied || slots[index].task_handle == rt::INVALID_HANDLE {
                continue;
            }

            let status = match rt::task_status(slots[index].task_handle) {
                Ok(status) => status,
                Err(_) => return 0xf611 + index as u64,
            };
            if status.state != TaskStateCode::Exited {
                continue;
            }

            let service_id = slots[index].manifest.service_id;
            let requested_restart = slots[index].restart_requested;
            slots[index].phase = ServicePhase::Exited;
            slots[index].last_exit_code = status.exit_code;

            if requested_restart {
                slots[index].restart_requested = false;
                if start_service(
                    slots,
                    *service_count,
                    index,
                    bootstrap_authority,
                    bootstrap_resource_for(service_id, bootstrap_resource),
                )
                .is_err()
                {
                    return 0xf620 + index as u64;
                }
                if wait_until_ready(slots, service_count, bootstrap_authority, service_id).is_err() {
                    return 0xf630 + index as u64;
                }
                continue;
            }

            if status.exit_code == 0 {
                continue;
            }

            let _ = emit_manager_event(
                slots,
                *service_count,
                LogSeverity::Error,
                LogEvent::ServiceFailed,
                service_id,
                status.exit_code,
            );

            let restart_limit = match slots[index].manifest.restart {
                RestartPolicy::OnFailure { max_restarts } => max_restarts,
            };
            if slots[index].attempts.saturating_sub(1) >= restart_limit {
                return 0xf640 + index as u64;
            }

            let _ = emit_manager_event(
                slots,
                *service_count,
                LogSeverity::Warn,
                LogEvent::ServiceRestarting,
                service_id,
                slots[index].attempts as u64 + 1,
            );

            if start_service(
                slots,
                *service_count,
                index,
                bootstrap_authority,
                bootstrap_resource_for(service_id, bootstrap_resource),
            )
            .is_err()
            {
                return 0xf650 + index as u64;
            }
            if wait_until_ready(slots, service_count, bootstrap_authority, service_id).is_err() {
                return 0xf660 + index as u64;
            }
        }

        if rt::yield_current().is_err() {
            return 0xf670;
        }
    }
}

fn bootstrap_resource_for(
    service_id: ServiceId,
    bootstrap_resource: (rt::Handle, usize),
) -> Option<(rt::Handle, usize)> {
    if service_id == ServiceId::Storage {
        Some(bootstrap_resource)
    } else {
        None
    }
}

fn pump_control_channels(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: &mut usize,
    bootstrap_authority: rt::Handle,
) -> rt::Result<()> {
    let mut index = 0usize;
    while index < *service_count {
        if !slots[index].occupied || slots[index].control_handle == rt::INVALID_HANDLE {
            index += 1;
            continue;
        }

        let mut message = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(slots[index].control_handle, &mut message) {
            Ok(()) => {
                handle_control_message(slots, service_count, index, bootstrap_authority, &message)?
            }
            Err(rt::Error::QueueEmpty) => {}
            Err(error) => return Err(error),
        }
        index += 1;
    }
    Ok(())
}

fn handle_control_message(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: &mut usize,
    service_index: usize,
    bootstrap_authority: rt::Handle,
    message: &RawMessage,
) -> rt::Result<()> {
    match message.tag {
        x if x == ControlTag::Register as u32 => {
            if message.word_count < 1 || message.handle_count < 1 {
                return Err(rt::Error::InvalidArgument);
            }
            let service_id = service_id_from_word(message.words[0]);
            if find_slot_index(slots, *service_count, service_id)? != service_index {
                return Err(rt::Error::PermissionDenied);
            }
            slots[service_index].public_handle = message.handles[0];
            slots[service_index].phase = ServicePhase::Ready;
            let _ = emit_manager_event(
                slots,
                *service_count,
                LogSeverity::Info,
                LogEvent::ServiceReady,
                service_id,
                slots[service_index].attempts as u64,
            );
        }
        x if x == ControlTag::LookupRequest as u32 => handle_lookup_request(slots, *service_count, service_index, message)?,
        x if x == ManagerTag::ListServicesRequest as u32 => {
            handle_list_services_request(slots, *service_count, service_index, message)?
        }
        x if x == ManagerTag::ServiceStatusRequest as u32 => {
            if message.word_count < 1 {
                return Err(rt::Error::InvalidArgument);
            }
            handle_service_status_request(slots, *service_count, service_index, message.words[0])?;
        }
        x if x == ManagerTag::ServiceActionRequest as u32 => {
            if message.word_count < 2 {
                return Err(rt::Error::InvalidArgument);
            }
            handle_service_action_request(
                slots,
                *service_count,
                service_index,
                message.words[0],
                message.words[1],
            )?;
        }
        x if x == ManagerTag::LaunchRequest as u32 => {
            if message.word_count < 1 {
                return Err(rt::Error::InvalidArgument);
            }
            handle_launch_request(
                slots,
                *service_count,
                service_index,
                bootstrap_authority,
                message,
            )?;
        }
        x if x == ManagerTag::ActivateRequest as u32 => {
            handle_activate_request(
                slots,
                service_count,
                service_index,
                bootstrap_authority,
                message,
            )?;
        }
        x if x == ManagerTag::DeactivateRequest as u32 => {
            if message.word_count < 1 {
                return Err(rt::Error::InvalidArgument);
            }
            handle_deactivate_request(slots, service_count, service_index, message.words[0])?;
        }
        _ => {}
    }

    Ok(())
}

fn handle_lookup_request(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    service_index: usize,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 1 {
        return Err(rt::Error::InvalidArgument);
    }

    let requested = service_id_from_word(message.words[0]);
    let permission = lookup_rights(slots[service_index].manifest, requested);
    let target = find_slot_index_checked(slots, service_count, requested).map(|index| &slots[index]);

    let mut reply = RawMessage::empty(ControlTag::LookupReply as u32);
    reply.word_count = 2;
    reply.words[0] = requested as u32 as u64;

    match (permission, target) {
        (Some(rights), Some(target))
            if target.phase == ServicePhase::Ready && target.public_handle != rt::INVALID_HANDLE =>
        {
            let duplicated =
                rt::handle_duplicate(target.public_handle, rights | rights::DUPLICATE | rights::TRANSFER)?;
            reply.words[1] = LookupStatus::Ok as u32 as u64;
            reply.handle_count = 1;
            reply.handles[0] = duplicated;
            reply.handle_rights[0] = rights;
            rt::channel_send(slots[service_index].control_handle, &reply)?;
            let _ = rt::handle_close(duplicated);
            let _ = emit_manager_event(
                slots,
                service_count,
                LogSeverity::Debug,
                LogEvent::LookupGranted,
                slots[service_index].manifest.service_id,
                requested as u32 as u64,
            );
        }
        (Some(_), _) => {
            reply.words[1] = LookupStatus::Unavailable as u32 as u64;
            rt::channel_send(slots[service_index].control_handle, &reply)?;
        }
        (None, _) => {
            reply.words[1] = LookupStatus::Denied as u32 as u64;
            rt::channel_send(slots[service_index].control_handle, &reply)?;
        }
    }

    Ok(())
}

fn handle_list_services_request(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    service_index: usize,
    message: &RawMessage,
) -> rt::Result<()> {
    let page_start = if message.word_count > 0 {
        message.words[0] as usize
    } else {
        0
    };

    let mut reply = RawMessage::empty(ManagerTag::ListServicesReply as u32);
    reply.word_count = 2;
    reply.words[0] = 0;
    reply.words[1] = u64::MAX;

    let mut visible_index = 0usize;
    let mut emitted = 0usize;
    let mut write_entry = |service_id: ServiceId, phase: ServicePhase, attempts: u32| {
        if visible_index < page_start {
            visible_index += 1;
            return;
        }
        if reply.word_count as usize + 2 > IPC_MAX_WORDS {
            reply.words[1] = visible_index as u64;
            return;
        }
        let base = reply.word_count as usize;
        reply.words[base] = service_id as u32 as u64;
        reply.words[base + 1] = encode_phase(phase, attempts);
        reply.word_count += 2;
        emitted += 1;
        visible_index += 1;
    };

    write_entry(ServiceId::RootManager, ServicePhase::Ready, 1);
    for slot in &slots[..service_count] {
        if slot.occupied {
            write_entry(slot.manifest.service_id, slot.phase, slot.attempts);
        }
    }

    reply.words[0] = emitted as u64;
    rt::channel_send(slots[service_index].control_handle, &reply)
}

fn handle_service_status_request(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    service_index: usize,
    requested_word: u64,
) -> rt::Result<()> {
    let requested = service_id_from_word(requested_word);
    let mut reply = RawMessage::empty(ManagerTag::ServiceStatusReply as u32);
    reply.word_count = 4;

    if requested == ServiceId::RootManager {
        reply.words[0] = ManagerStatus::Ok as u32 as u64;
        reply.words[1] = ManagerServicePhase::Ready as u32 as u64;
        reply.words[2] = 1;
        reply.words[3] = 0;
        return rt::channel_send(slots[service_index].control_handle, &reply);
    }

    let Some(target_index) = find_slot_index_checked(slots, service_count, requested) else {
        reply.words[0] = ManagerStatus::NotFound as u32 as u64;
        return rt::channel_send(slots[service_index].control_handle, &reply);
    };
    let slot = &slots[target_index];
    reply.words[0] = ManagerStatus::Ok as u32 as u64;
    reply.words[1] = manager_phase(slot.phase) as u32 as u64;
    reply.words[2] = slot.attempts as u64;
    reply.words[3] = slot.last_exit_code;
    rt::channel_send(slots[service_index].control_handle, &reply)
}

fn handle_service_action_request(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    service_index: usize,
    requested_word: u64,
    action_word: u64,
) -> rt::Result<()> {
    let mut reply = RawMessage::empty(ManagerTag::ServiceActionReply as u32);
    reply.word_count = 1;
    if slots[service_index].manifest.service_id != ServiceId::Shell {
        reply.words[0] = ManagerStatus::Denied as u32 as u64;
        return rt::channel_send(slots[service_index].control_handle, &reply);
    }

    let requested = service_id_from_word(requested_word);
    let action = manager_action_from_word(action_word);
    let Some(target_index) = find_slot_index_checked(slots, service_count, requested) else {
        reply.words[0] = ManagerStatus::NotFound as u32 as u64;
        return rt::channel_send(slots[service_index].control_handle, &reply);
    };
    if matches!(requested, ServiceId::Shell | ServiceId::Package) || action != ManagerAction::Restart {
        reply.words[0] = ManagerStatus::Denied as u32 as u64;
        return rt::channel_send(slots[service_index].control_handle, &reply);
    }
    if slots[target_index].phase != ServicePhase::Ready || slots[target_index].restart_requested {
        reply.words[0] = ManagerStatus::Busy as u32 as u64;
        return rt::channel_send(slots[service_index].control_handle, &reply);
    }

    reply.words[0] = ManagerStatus::Ok as u32 as u64;
    rt::channel_send(slots[service_index].control_handle, &reply)?;
    send_lifecycle(slots[target_index].control_handle, LifecycleEvent::Restarting)?;
    slots[target_index].restart_requested = true;
    let _ = emit_manager_event(
        slots,
        service_count,
        LogSeverity::Warn,
        LogEvent::ServiceRestarting,
        slots[target_index].manifest.service_id,
        slots[target_index].attempts as u64 + 1,
    );
    Ok(())
}

fn handle_launch_request(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    service_index: usize,
    bootstrap_authority: rt::Handle,
    message: &RawMessage,
) -> rt::Result<()> {
    let mut reply = RawMessage::empty(ManagerTag::LaunchReply as u32);
    reply.word_count = 1;
    if slots[service_index].manifest.service_id != ServiceId::Shell {
        reply.words[0] = ManagerStatus::Denied as u32 as u64;
        return rt::channel_send(slots[service_index].control_handle, &reply);
    }

    let image_id = image_id_from_word(message.words[0]);
    if image_id != ServiceImageId::SysinfoTool {
        reply.words[0] = ManagerStatus::NotFound as u32 as u64;
        return rt::channel_send(slots[service_index].control_handle, &reply);
    }

    let io_handle = if message.handle_count > 0 {
        Some(message.handles[0])
    } else {
        None
    };
    let task_handle = match launch_program(bootstrap_authority, image_id, io_handle) {
        Ok(task_handle) => task_handle,
        Err(rt::Error::NotFound) => {
            reply.words[0] = ManagerStatus::NotFound as u32 as u64;
            return rt::channel_send(slots[service_index].control_handle, &reply);
        }
        Err(rt::Error::PermissionDenied) => {
            reply.words[0] = ManagerStatus::Denied as u32 as u64;
            return rt::channel_send(slots[service_index].control_handle, &reply);
        }
        Err(_) => {
            reply.words[0] = ManagerStatus::Busy as u32 as u64;
            return rt::channel_send(slots[service_index].control_handle, &reply);
        }
    };

    reply.words[0] = ManagerStatus::Ok as u32 as u64;
    reply.handle_count = 1;
    reply.handles[0] = task_handle;
    reply.handle_rights[0] = rights::READ;
    rt::channel_send(slots[service_index].control_handle, &reply)?;
    let _ = rt::handle_close(task_handle);
    let _ = emit_manager_event(
        slots,
        service_count,
        LogSeverity::Info,
        LogEvent::ToolLaunched,
        ServiceId::Shell,
        image_id as u32 as u64,
    );
    Ok(())
}

fn handle_activate_request(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: &mut usize,
    service_index: usize,
    bootstrap_authority: rt::Handle,
    message: &RawMessage,
) -> rt::Result<()> {
    let control_handle = slots[service_index].control_handle;
    let mut reply = RawMessage::empty(ManagerTag::ActivateReply as u32);
    reply.word_count = 2;
    reply.words[0] = ManagerStatus::Denied as u32 as u64;
    reply.words[1] = ServiceId::RootManager as u32 as u64;

    if slots[service_index].manifest.service_id != ServiceId::Package || message.word_count < 1 {
        return rt::channel_send(control_handle, &reply);
    }

    let path_len = message.words[0] as usize;
    let mut path_bytes = [0u8; BOOT_STORE_PATH_MAX];
    if unpack_bytes(
        &message.words[1..message.word_count as usize],
        path_len,
        &mut path_bytes,
    )
    .is_err()
    {
        reply.words[0] = ManagerStatus::Failed as u32 as u64;
        return rt::channel_send(control_handle, &reply);
    }
    let path = core::str::from_utf8(&path_bytes[..path_len]).map_err(|_| rt::Error::InvalidArgument)?;

    let manifest = match load_manifest_from_storage(slots, *service_count, path) {
        Ok(manifest) => manifest,
        Err(rt::Error::NotFound) => {
            reply.words[0] = ManagerStatus::NotFound as u32 as u64;
            return rt::channel_send(control_handle, &reply);
        }
        Err(_) => {
            reply.words[0] = ManagerStatus::Failed as u32 as u64;
            return rt::channel_send(control_handle, &reply);
        }
    };
    reply.words[1] = manifest.service_id as u32 as u64;

    let target_index = if let Some(index) = find_slot_index_checked(slots, *service_count, manifest.service_id) {
        if !slots[index].dynamic {
            return rt::channel_send(control_handle, &reply);
        }
        stop_service_slot(&mut slots[index])?;
        index
    } else {
        allocate_slot(slots, service_count)?
    };

    slots[target_index] = ServiceSlot {
        manifest,
        occupied: true,
        dynamic: true,
        ..ServiceSlot::empty()
    };

    let result = start_service(slots, *service_count, target_index, bootstrap_authority, None)
        .and_then(|_| wait_until_ready(slots, service_count, bootstrap_authority, manifest.service_id));

    reply.words[0] = if result.is_ok() {
        ManagerStatus::Ok as u32 as u64
    } else {
        let exit_code = rt::task_status(slots[target_index].task_handle)
            .map(|status| status.exit_code)
            .unwrap_or(0);
        let _ = emit_manager_event(
            slots,
            *service_count,
            LogSeverity::Error,
            LogEvent::ServiceFailed,
            manifest.service_id,
            exit_code,
        );
        let _ = close_slot_for_failure(&mut slots[target_index]);
        ManagerStatus::Failed as u32 as u64
    };
    rt::channel_send(control_handle, &reply)
}

fn handle_deactivate_request(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: &mut usize,
    service_index: usize,
    requested_word: u64,
) -> rt::Result<()> {
    let control_handle = slots[service_index].control_handle;
    let mut reply = RawMessage::empty(ManagerTag::DeactivateReply as u32);
    reply.word_count = 1;
    reply.words[0] = ManagerStatus::Denied as u32 as u64;

    if slots[service_index].manifest.service_id != ServiceId::Package {
        return rt::channel_send(control_handle, &reply);
    }

    let requested = service_id_from_word(requested_word);
    let Some(target_index) = find_slot_index_checked(slots, *service_count, requested) else {
        reply.words[0] = ManagerStatus::NotFound as u32 as u64;
        return rt::channel_send(control_handle, &reply);
    };
    if !slots[target_index].dynamic {
        return rt::channel_send(control_handle, &reply);
    }

    reply.words[0] = if stop_service_slot(&mut slots[target_index]).is_ok() {
        slots[target_index] = ServiceSlot::empty();
        compact_service_slots(slots, service_count);
        ManagerStatus::Ok as u32 as u64
    } else {
        ManagerStatus::Failed as u32 as u64
    };
    rt::channel_send(control_handle, &reply)
}

fn launch_program(
    bootstrap_authority: rt::Handle,
    image_id: ServiceImageId,
    io_handle: Option<rt::Handle>,
) -> rt::Result<rt::Handle> {
    let bootstrap = rt::channel_create()?;
    let task_handle = rt::service_spawn(image_id, bootstrap_authority, bootstrap.second)?;
    let task_view =
        rt::handle_duplicate(task_handle, rights::READ | rights::DUPLICATE | rights::TRANSFER)?;

    let mut startup = RawMessage::empty(ControlTag::Startup as u32);
    startup.word_count = 1;
    startup.words[0] = u64::from(io_handle.is_some());
    if let Some(io_handle) = io_handle {
        startup.handle_count = 1;
        startup.handles[0] = io_handle;
        startup.handle_rights[0] = rights::SEND | rights::RECEIVE;
    }
    rt::channel_send(bootstrap.first, &startup)?;
    if startup.handle_count > 0 {
        let _ = rt::handle_close(startup.handles[0]);
    }
    let _ = rt::handle_close(task_handle);
    let _ = rt::handle_close(bootstrap.first);
    Ok(task_view)
}

fn load_manifest_from_storage(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    path: &str,
) -> rt::Result<ServiceManifest> {
    let storage_index = find_slot_index(slots, service_count, ServiceId::Storage)?;
    let storage_handle = slots[storage_index].public_handle;
    let (manifest_handle, manifest_len) = rt::storage_open(storage_handle, path)?;
    let mut manifest_buffer = [0u8; MAX_MANIFEST_BYTES];
    let requested = manifest_len.min(manifest_buffer.len());
    let loaded = rt::storage_read_all(
        manifest_handle,
        &mut manifest_buffer,
        requested,
    )?;
    let _ = rt::storage_blob_close(manifest_handle);
    parse_manifest(&manifest_buffer[..loaded]).map_err(|_| rt::Error::InvalidArgument)
}

fn stop_service_slot(slot: &mut ServiceSlot) -> rt::Result<()> {
    if slot.control_handle != rt::INVALID_HANDLE {
        let _ = send_lifecycle(slot.control_handle, LifecycleEvent::Stopped);
    }
    if slot.task_handle != rt::INVALID_HANDLE {
        loop {
            match rt::task_status(slot.task_handle) {
                Ok(status) if status.state == TaskStateCode::Exited => {
                    slot.last_exit_code = status.exit_code;
                    break;
                }
                Ok(_) => rt::yield_current()?,
                Err(_) => break,
            }
        }
    }
    close_slot_handles(slot);
    slot.phase = ServicePhase::Exited;
    slot.restart_requested = false;
    Ok(())
}

fn close_slot_for_failure(slot: &mut ServiceSlot) -> rt::Result<()> {
    stop_service_slot(slot)?;
    Ok(())
}

fn send_lifecycle(control_handle: rt::Handle, event: LifecycleEvent) -> rt::Result<()> {
    let mut message = RawMessage::empty(ControlTag::Lifecycle as u32);
    message.word_count = 1;
    message.words[0] = event as u32 as u64;
    rt::channel_send(control_handle, &message)
}

fn emit_manager_event(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    severity: LogSeverity,
    event: LogEvent,
    target: ServiceId,
    detail: u64,
) -> rt::Result<()> {
    if let Some(log_index) = find_slot_index_checked(slots, service_count, ServiceId::Log) {
        let log_handle = slots[log_index].public_handle;
        if log_handle != rt::INVALID_HANDLE {
            return rt::send_log_record(
                log_handle,
                ServiceId::RootManager,
                severity,
                LogDomain::ServiceManager,
                event,
                target as u32 as u64,
                detail,
            );
        }
    }
    fallback_manager_event(severity, event, target, detail)
}

fn fallback_manager_event(
    severity: LogSeverity,
    event: LogEvent,
    target: ServiceId,
    detail: u64,
) -> rt::Result<()> {
    rt::write_logf(
        "service-manager",
        format_args!(
            "level={} event={} target={} detail={}",
            severity_name(severity),
            event_name(event),
            service_name(target),
            detail,
        ),
    )
}

fn dependencies_ready(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    index: usize,
) -> bool {
    slots[index].manifest.dependencies[..slots[index].manifest.dependency_count]
        .iter()
        .copied()
        .all(|dependency| {
            if dependency == ServiceId::RootManager {
                return true;
            }
            find_slot_index_checked(slots, service_count, dependency)
                .map(|slot| slots[slot].phase == ServicePhase::Ready)
                .unwrap_or(false)
        })
}

fn lookup_rights(manifest: ServiceManifest, requested: ServiceId) -> Option<u64> {
    manifest.lookups[..manifest.lookup_count]
        .iter()
        .find(|entry| entry.target == requested)
        .map(|entry| entry.rights)
}

fn allocate_slot(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: &mut usize,
) -> rt::Result<usize> {
    if let Some(index) = (0..*service_count).find(|index| !slots[*index].occupied) {
        return Ok(index);
    }
    if *service_count == slots.len() {
        return Err(rt::Error::CapacityExceeded);
    }
    let index = *service_count;
    *service_count += 1;
    Ok(index)
}

fn compact_service_slots(slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS], service_count: &mut usize) {
    while *service_count > 0 && !slots[*service_count - 1].occupied {
        *service_count -= 1;
    }
}

fn occupied_service_count(slots: &[ServiceSlot; MAX_SERVICE_SLOTS], service_count: usize) -> usize {
    slots[..service_count].iter().filter(|slot| slot.occupied).count()
}

fn ready_service_count(slots: &[ServiceSlot; MAX_SERVICE_SLOTS], service_count: usize) -> usize {
    slots[..service_count]
        .iter()
        .filter(|slot| slot.occupied && slot.phase == ServicePhase::Ready)
        .count()
}

fn close_slot_handles(slot: &mut ServiceSlot) {
    close_if_valid(&mut slot.task_handle);
    close_if_valid(&mut slot.control_handle);
    close_if_valid(&mut slot.public_handle);
}

fn close_if_valid(handle: &mut rt::Handle) {
    if *handle != rt::INVALID_HANDLE {
        let _ = rt::handle_close(*handle);
        *handle = rt::INVALID_HANDLE;
    }
}

fn find_slot_index(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    service_id: ServiceId,
) -> rt::Result<usize> {
    find_slot_index_checked(slots, service_count, service_id).ok_or(rt::Error::NotFound)
}

fn find_slot_index_checked(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    service_id: ServiceId,
) -> Option<usize> {
    (0..service_count).find(|index| slots[*index].occupied && slots[*index].manifest.service_id == service_id)
}

fn unpack_bytes(words: &[u64], len: usize, destination: &mut [u8]) -> rt::Result<()> {
    if len > destination.len() || len > words.len() * 8 {
        return Err(rt::Error::BufferTooSmall);
    }

    let mut copied = 0usize;
    for word in words.iter().copied() {
        if copied >= len {
            break;
        }
        let bytes = word.to_le_bytes();
        let chunk = (len - copied).min(bytes.len());
        destination[copied..copied + chunk].copy_from_slice(&bytes[..chunk]);
        copied += chunk;
    }
    Ok(())
}

fn service_id_from_word(value: u64) -> ServiceId {
    match value as u32 {
        x if x == ServiceId::Storage as u32 => ServiceId::Storage,
        x if x == ServiceId::Console as u32 => ServiceId::Console,
        x if x == ServiceId::Config as u32 => ServiceId::Config,
        x if x == ServiceId::Log as u32 => ServiceId::Log,
        x if x == ServiceId::Status as u32 => ServiceId::Status,
        x if x == ServiceId::Shell as u32 => ServiceId::Shell,
        x if x == ServiceId::Package as u32 => ServiceId::Package,
        x if x == ServiceId::Announce as u32 => ServiceId::Announce,
        _ => ServiceId::RootManager,
    }
}

fn image_id_from_word(value: u64) -> ServiceImageId {
    match value as u32 {
        x if x == ServiceImageId::StorageService as u32 => ServiceImageId::StorageService,
        x if x == ServiceImageId::ConsoleService as u32 => ServiceImageId::ConsoleService,
        x if x == ServiceImageId::ConfigService as u32 => ServiceImageId::ConfigService,
        x if x == ServiceImageId::LogService as u32 => ServiceImageId::LogService,
        x if x == ServiceImageId::StatusService as u32 => ServiceImageId::StatusService,
        x if x == ServiceImageId::ShellService as u32 => ServiceImageId::ShellService,
        x if x == ServiceImageId::SysinfoTool as u32 => ServiceImageId::SysinfoTool,
        x if x == ServiceImageId::PackageService as u32 => ServiceImageId::PackageService,
        x if x == ServiceImageId::AnnounceService as u32 => ServiceImageId::AnnounceService,
        _ => ServiceImageId::RootManager,
    }
}

fn manager_action_from_word(value: u64) -> ManagerAction {
    match value as u32 {
        x if x == ManagerAction::Restart as u32 => ManagerAction::Restart,
        _ => ManagerAction::Restart,
    }
}

fn encode_phase(phase: ServicePhase, attempts: u32) -> u64 {
    manager_phase(phase) as u32 as u64 | ((attempts as u64) << 32)
}

fn manager_phase(phase: ServicePhase) -> ManagerServicePhase {
    match phase {
        ServicePhase::Dormant => ManagerServicePhase::Dormant,
        ServicePhase::Starting => ManagerServicePhase::Starting,
        ServicePhase::Ready => ManagerServicePhase::Ready,
        ServicePhase::Exited => ManagerServicePhase::Exited,
    }
}

fn service_name(service_id: ServiceId) -> &'static str {
    match service_id {
        ServiceId::RootManager => "root-manager",
        ServiceId::Storage => "storage-service",
        ServiceId::Console => "console-service",
        ServiceId::Config => "config-service",
        ServiceId::Log => "log-service",
        ServiceId::Status => "status-service",
        ServiceId::Shell => "shell-service",
        ServiceId::Package => "package-service",
        ServiceId::Announce => "announce-service",
    }
}

fn severity_name(severity: LogSeverity) -> &'static str {
    match severity {
        LogSeverity::Trace => "trace",
        LogSeverity::Debug => "debug",
        LogSeverity::Info => "info",
        LogSeverity::Warn => "warn",
        LogSeverity::Error => "error",
    }
}

fn event_name(event: LogEvent) -> &'static str {
    match event {
        LogEvent::ServiceStarted => "service-started",
        LogEvent::ServiceReady => "service-ready",
        LogEvent::ServiceFailed => "service-failed",
        LogEvent::ServiceRestarting => "service-restarting",
        LogEvent::ConfigLoaded => "config-loaded",
        LogEvent::ConfigRead => "config-read",
        LogEvent::ConsoleWrite => "console-write",
        LogEvent::StatusStarted => "status-started",
        LogEvent::StatusHeartbeat => "status-heartbeat",
        LogEvent::LookupGranted => "lookup-granted",
        LogEvent::StorageMounted => "storage-mounted",
        LogEvent::ManifestLoaded => "manifest-loaded",
        LogEvent::ResourceOpened => "resource-opened",
        LogEvent::SessionOpened => "session-opened",
        LogEvent::ShellCommand => "shell-command",
        LogEvent::ToolLaunched => "tool-launched",
        LogEvent::PackageCatalogLoaded => "package-catalog-loaded",
        LogEvent::PackageInstalled => "package-installed",
        LogEvent::PackageUpdated => "package-updated",
        LogEvent::PackageRemoved => "package-removed",
        LogEvent::PackageRolledBack => "package-rolled-back",
        LogEvent::PackageActivationFailed => "package-activation-failed",
    }
}

fn fallback_log(message: &str) {
    let _ = rt::write_log("service-manager", message);
}

fn fallback_logf(args: core::fmt::Arguments<'_>) -> rt::Result<()> {
    rt::write_logf("service-manager", args)
}
