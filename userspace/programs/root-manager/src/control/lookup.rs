use serviceos_userspace_runtime as rt;
use rt::{
    LifecycleEvent, LogEvent, LogSeverity, LookupStatus, ManagerAction, ManagerStatus, ManagerTag,
    RawMessage, ServiceId,
};

use crate::{
    state::{ServicePhase, ServiceSlot, MAX_SERVICE_SLOTS},
    util::{
        emit_manager_event, encode_phase, find_slot_index_checked, lookup_rights,
        manager_action_from_word, manager_phase, service_id_from_word,
    },
};

use super::lifecycle::send_lifecycle;

pub(super) fn handle_lookup_request(
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

    let mut reply = RawMessage::empty(rt::ControlTag::LookupReply as u32);
    reply.word_count = 2;
    reply.words[0] = requested as u32 as u64;

    match (permission, target) {
        (Some(rights), Some(target))
            if target.phase == ServicePhase::Ready && target.public_handle != rt::INVALID_HANDLE =>
        {
            let duplicated = rt::handle_duplicate(
                target.public_handle,
                rights | rt::rights::DUPLICATE | rt::rights::TRANSFER,
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

pub(super) fn handle_list_services_request(
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
        if reply.word_count as usize + 2 > rt::IPC_MAX_WORDS {
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

pub(super) fn handle_service_status_request(
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

pub(super) fn handle_service_action_request(
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
