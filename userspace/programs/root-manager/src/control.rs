use serviceos_bundle::{BOOT_STORE_PATH_MAX, parse_manifest};
use serviceos_userspace_runtime as rt;
use rt::{
    ControlTag, LifecycleEvent, LogEvent, LogSeverity, LookupStatus, ManagerAction,
    ManagerStatus, ManagerTag, RawMessage, ServiceId, ServiceImageId, TaskStateCode,
    IPC_MAX_HANDLES, IPC_MAX_WORDS, rights,
};

use crate::graph::{start_service, wait_until_ready};
use crate::state::{ServicePhase, ServiceSlot, MAX_MANIFEST_BYTES, MAX_SERVICE_SLOTS};
use crate::util::{
    allocate_slot, compact_service_slots, emit_manager_event, encode_phase, find_slot_index,
    find_slot_index_checked, lookup_rights, manager_action_from_word, manager_phase,
    service_id_from_word, unpack_bytes, image_id_from_word,
};

pub(crate) fn pump_control_channels(
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
        x if x == ControlTag::LookupRequest as u32 => {
            handle_lookup_request(slots, *service_count, service_index, message)?
        }
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
            let duplicated = rt::handle_duplicate(
                target.public_handle,
                rights | rights::DUPLICATE | rights::TRANSFER,
            )?;
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
        reply.words[1] = rt::ManagerServicePhase::Ready as u32 as u64;
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
    if matches!(requested, ServiceId::Shell | ServiceId::Package)
        || action != ManagerAction::Restart
    {
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
    let caller = slots[service_index].manifest.service_id;
    if caller != ServiceId::Shell && caller != ServiceId::DesktopShell {
        reply.words[0] = ManagerStatus::Denied as u32 as u64;
        let result = rt::channel_send(slots[service_index].control_handle, &reply);
        close_message_handles(message);
        return result;
    }

    let image_id = image_id_from_word(message.words[0]);
    if !launch_is_authorized(caller, image_id) {
        reply.words[0] = ManagerStatus::NotFound as u32 as u64;
        let result = rt::channel_send(slots[service_index].control_handle, &reply);
        close_message_handles(message);
        return result;
    }

    let startup_word_count = if message.word_count > 1 {
        (message.words[1] as usize).min(IPC_MAX_WORDS.saturating_sub(2))
    } else {
        0
    };
    if (message.word_count as usize) < 2 + startup_word_count {
        reply.words[0] = ManagerStatus::Failed as u32 as u64;
        let result = rt::channel_send(slots[service_index].control_handle, &reply);
        close_message_handles(message);
        return result;
    }
    let task_handle = match launch_program(
        slots,
        service_count,
        bootstrap_authority,
        caller,
        image_id,
        &message.words[2..2 + startup_word_count],
        &message.handles[..message.handle_count as usize],
        &message.handle_rights[..message.handle_count as usize],
    ) {
        Ok(task_handle) => task_handle,
        Err(rt::Error::NotFound) => {
            reply.words[0] = ManagerStatus::NotFound as u32 as u64;
            let result = rt::channel_send(slots[service_index].control_handle, &reply);
            close_message_handles(message);
            return result;
        }
        Err(rt::Error::PermissionDenied) => {
            reply.words[0] = ManagerStatus::Denied as u32 as u64;
            let result = rt::channel_send(slots[service_index].control_handle, &reply);
            close_message_handles(message);
            return result;
        }
        Err(_) => {
            reply.words[0] = ManagerStatus::Busy as u32 as u64;
            let result = rt::channel_send(slots[service_index].control_handle, &reply);
            close_message_handles(message);
            return result;
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
        caller,
        image_id as u32 as u64,
    );
    Ok(())
}

fn close_message_handles(message: &RawMessage) {
    for handle in message.handles[..message.handle_count as usize]
        .iter()
        .copied()
    {
        let _ = rt::handle_close(handle);
    }
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
    let task_view =
        rt::handle_duplicate(task_handle, rights::READ | rights::DUPLICATE | rights::TRANSFER)?;

    let mut startup = RawMessage::empty(ControlTag::Startup as u32);
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
    for handle in startup.handles[..startup.handle_count as usize]
        .iter()
        .copied()
    {
        let _ = rt::handle_close(handle);
    }
    let _ = rt::handle_close(task_handle);
    let _ = rt::handle_close(bootstrap.first);
    Ok(task_view)
}

fn launch_is_authorized(caller: ServiceId, image_id: ServiceImageId) -> bool {
    match caller {
        ServiceId::Shell | ServiceId::Terminal => image_id == ServiceImageId::SysinfoTool,
        ServiceId::DesktopShell => matches!(
            image_id,
            ServiceImageId::SettingsApp
                | ServiceImageId::FilesApp
                | ServiceImageId::MonitorApp
                | ServiceImageId::TerminalApp
        ),
        _ => false,
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
        }
        _ => {}
    }

    Ok(())
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
    let transferred = rt::handle_duplicate(
        slots[index].public_handle,
        rights_mask | rights::DUPLICATE,
    )?;
    startup.handles[*handle_index] = transferred;
    startup.handle_rights[*handle_index] = rights_mask & !rights::TRANSFER;
    *handle_index += 1;
    Ok(())
}

pub(crate) fn load_manifest_from_storage(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    path: &str,
) -> rt::Result<serviceos_bundle::ServiceManifest> {
    let storage_index = find_slot_index(slots, service_count, ServiceId::Storage)?;
    let storage_handle = slots[storage_index].public_handle;
    let (manifest_handle, manifest_len) = rt::storage_open(storage_handle, path)?;
    let mut manifest_buffer = [0u8; MAX_MANIFEST_BYTES];
    let requested = manifest_len.min(manifest_buffer.len());
    let loaded = rt::storage_read_all(manifest_handle, &mut manifest_buffer, requested)?;
    let _ = rt::storage_blob_close(manifest_handle);
    parse_manifest(&manifest_buffer[..loaded]).map_err(|_| rt::Error::InvalidArgument)
}

pub(crate) fn stop_service_slot(slot: &mut ServiceSlot) -> rt::Result<()> {
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
    crate::util::close_slot_handles(slot);
    slot.phase = ServicePhase::Exited;
    slot.restart_requested = false;
    Ok(())
}

pub(crate) fn close_slot_for_failure(slot: &mut ServiceSlot) -> rt::Result<()> {
    stop_service_slot(slot)?;
    Ok(())
}

fn send_lifecycle(control_handle: rt::Handle, event: LifecycleEvent) -> rt::Result<()> {
    let mut message = RawMessage::empty(ControlTag::Lifecycle as u32);
    message.word_count = 1;
    message.words[0] = event as u32 as u64;
    rt::channel_send(control_handle, &message)
}
