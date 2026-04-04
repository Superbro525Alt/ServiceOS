use serviceos_userspace_runtime as rt;
use rt::{
    LifecycleEvent, LogEvent, LogSeverity, LookupStatus, ManagerAction, ManagerAvailability,
    ManagerLookupPolicy, ManagerStartupMode, ManagerStatus, ManagerTag, RawMessage, ServiceId,
};

use crate::{
    graph::ensure_service_ready,
    state::{BootstrapResources, GraphStatus, ServicePhase, ServiceSlot, MAX_SERVICE_SLOTS},
    util::{
        emit_manager_event, encode_phase, find_slot_index_checked, lookup_rights,
        manager_action_from_word, manager_phase, service_availability, service_id_from_word,
        service_startup_mode, set_lookup_policy,
    },
};

use super::lifecycle::send_lifecycle;

pub(super) fn handle_lookup_request(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: &mut usize,
    service_index: usize,
    bootstrap_authority: rt::Handle,
    bootstrap_resources: BootstrapResources,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 1 {
        return Err(rt::Error::InvalidArgument);
    }

    let requested = service_id_from_word(message.words[0]);
    let permission = lookup_rights(&slots[service_index], requested);
    let target_index = find_slot_index_checked(slots, *service_count, requested);

    let mut reply = RawMessage::empty(rt::ControlTag::LookupReply as u32);
    reply.word_count = 2;
    reply.words[0] = requested as u32 as u64;

    match (permission, target_index) {
        (Some(rights), Some(target_index)) => {
            if slots[target_index].phase != ServicePhase::Ready
                && slots[target_index].manifest.startup == serviceos_bundle::ServiceStartupMode::OnDemand
            {
                let _ = ensure_service_ready(
                    slots,
                    service_count,
                    bootstrap_authority,
                    bootstrap_resources,
                    requested,
                );
            }

            if slots[target_index].phase == ServicePhase::Ready
                && slots[target_index].public_handle != rt::INVALID_HANDLE
            {
                let duplicated = rt::handle_duplicate(
                    slots[target_index].public_handle,
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
                    *service_count,
                    LogSeverity::Debug,
                    LogEvent::LookupGranted,
                    slots[service_index].manifest.service_id,
                    requested as u32 as u64,
                );
            } else {
                reply.words[1] = LookupStatus::Unavailable as u32 as u64;
                rt::channel_send(slots[service_index].control_handle, &reply)?;
            }
        }
        (Some(_), None) => {
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

pub(super) fn handle_service_lookup_list_request(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    service_index: usize,
    requested_word: u64,
    page_start: usize,
) -> rt::Result<()> {
    let requested = service_id_from_word(requested_word);
    let mut reply = RawMessage::empty(ManagerTag::ServiceLookupListReply as u32);
    reply.word_count = 3;
    reply.words[0] = ManagerStatus::Ok as u32 as u64;
    reply.words[1] = 0;
    reply.words[2] = u64::MAX;

    let Some(target_index) = find_slot_index_checked(slots, service_count, requested) else {
        reply.word_count = 1;
        reply.words[0] = ManagerStatus::NotFound as u32 as u64;
        return rt::channel_send(slots[service_index].control_handle, &reply);
    };
    let slot = &slots[target_index];

    let mut visible_index = 0usize;
    let mut emitted = 0usize;
    for (index, entry) in slot.manifest.lookups[..slot.manifest.lookup_count]
        .iter()
        .copied()
        .enumerate()
    {
        if visible_index < page_start {
            visible_index += 1;
            continue;
        }
        if reply.word_count as usize + 3 > rt::IPC_MAX_WORDS {
            reply.words[2] = visible_index as u64;
            break;
        }
        let base = reply.word_count as usize;
        reply.words[base] = entry.target as u32 as u64;
        reply.words[base + 1] = entry.rights;
        reply.words[base + 2] = if (slot.revoked_lookup_mask & (1u64 << index)) != 0 {
            ManagerLookupPolicy::Revoked as u32 as u64
        } else {
            ManagerLookupPolicy::Default as u32 as u64
        };
        reply.word_count += 3;
        emitted += 1;
        visible_index += 1;
    }

    reply.words[1] = emitted as u64;
    rt::channel_send(slots[service_index].control_handle, &reply)
}

pub(super) fn handle_service_lookup_policy_set_request(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    service_index: usize,
    requested_word: u64,
    target_word: u64,
    policy_word: u64,
) -> rt::Result<()> {
    let mut reply = RawMessage::empty(ManagerTag::ServiceLookupPolicySetReply as u32);
    reply.word_count = 1;
    if slots[service_index].manifest.service_id != ServiceId::Shell {
        reply.words[0] = ManagerStatus::Denied as u32 as u64;
        return rt::channel_send(slots[service_index].control_handle, &reply);
    }

    let requested = service_id_from_word(requested_word);
    let target = service_id_from_word(target_word);
    let policy = match policy_word as u32 {
        x if x == ManagerLookupPolicy::Default as u32 => ManagerLookupPolicy::Default,
        x if x == ManagerLookupPolicy::Revoked as u32 => ManagerLookupPolicy::Revoked,
        _ => {
            reply.words[0] = ManagerStatus::Failed as u32 as u64;
            return rt::channel_send(slots[service_index].control_handle, &reply);
        }
    };

    let Some(target_index) = find_slot_index_checked(slots, service_count, requested) else {
        reply.words[0] = ManagerStatus::NotFound as u32 as u64;
        return rt::channel_send(slots[service_index].control_handle, &reply);
    };
    if !set_lookup_policy(&mut slots[target_index], target, policy) {
        reply.words[0] = ManagerStatus::NotFound as u32 as u64;
        return rt::channel_send(slots[service_index].control_handle, &reply);
    }

    reply.words[0] = ManagerStatus::Ok as u32 as u64;
    rt::channel_send(slots[service_index].control_handle, &reply)?;
    let _ = emit_manager_event(
        slots,
        service_count,
        LogSeverity::Warn,
        LogEvent::SecurityPolicyChanged,
        requested,
        target as u32 as u64,
    );
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
    reply.word_count = 10;

    if requested == ServiceId::RootManager {
        reply.words[0] = ManagerStatus::Ok as u32 as u64;
        reply.words[1] = rt::ManagerServicePhase::Ready as u32 as u64;
        reply.words[2] = 1;
        reply.words[3] = 0;
        reply.words[4] = ManagerStartupMode::Eager as u32 as u64;
        reply.words[5] = ManagerAvailability::Required as u32 as u64;
        reply.words[6] = ServiceId::RootManager as u32 as u64;
        reply.words[7] = 0;
        reply.words[8] = 0;
        reply.words[9] = 0;
        return rt::channel_send(slots[service_index].control_handle, &reply);
    }

    let Some(target_index) = find_slot_index_checked(slots, service_count, requested) else {
        reply.word_count = 1;
        reply.words[0] = ManagerStatus::NotFound as u32 as u64;
        return rt::channel_send(slots[service_index].control_handle, &reply);
    };
    let slot = &slots[target_index];
    reply.words[0] = ManagerStatus::Ok as u32 as u64;
    reply.words[1] = manager_phase(slot.phase) as u32 as u64;
    reply.words[2] = slot.attempts as u64;
    reply.words[3] = slot.last_exit_code;
    reply.words[4] = service_startup_mode(slot.manifest) as u32 as u64;
    reply.words[5] = service_availability(slot.manifest) as u32 as u64;
    reply.words[6] = slot.blocked_dependency as u32 as u64;
    reply.words[7] = slot.last_start_tick;
    reply.words[8] = slot.last_ready_tick;
    reply.words[9] = slot.next_restart_tick;
    rt::channel_send(slots[service_index].control_handle, &reply)
}

pub(super) fn handle_service_template_request(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    service_index: usize,
    requested_word: u64,
) -> rt::Result<()> {
    let requested = service_id_from_word(requested_word);
    let mut reply = RawMessage::empty(ManagerTag::ServiceTemplateReply as u32);
    reply.word_count = 8;

    if requested == ServiceId::RootManager {
        reply.words[0] = ManagerStatus::Ok as u32 as u64;
        reply.words[1] = ManagerStartupMode::Eager as u32 as u64;
        reply.words[2] = ManagerAvailability::Required as u32 as u64;
        reply.words[3] = 500;
        reply.words[4] = 0;
        reply.words[5] = 0;
        reply.words[6] = 0;
        reply.words[7] = 0;
        return rt::channel_send(slots[service_index].control_handle, &reply);
    }

    let Some(target_index) = find_slot_index_checked(slots, service_count, requested) else {
        reply.word_count = 1;
        reply.words[0] = ManagerStatus::NotFound as u32 as u64;
        return rt::channel_send(slots[service_index].control_handle, &reply);
    };
    let manifest = slots[target_index].manifest;
    let (restart_limit, restart_backoff) = match manifest.restart {
        serviceos_bundle::RestartPolicy::OnFailure {
            max_restarts,
            backoff_ticks,
        } => (max_restarts, backoff_ticks),
    };
    reply.words[0] = ManagerStatus::Ok as u32 as u64;
    reply.words[1] = service_startup_mode(manifest) as u32 as u64;
    reply.words[2] = service_availability(manifest) as u32 as u64;
    reply.words[3] = manifest.ready_timeout_ticks as u64;
    reply.words[4] = restart_limit as u64;
    reply.words[5] = restart_backoff as u64;
    reply.words[6] = manifest.grant_count as u64;
    reply.words[7] = manifest.lookup_count as u64;
    rt::channel_send(slots[service_index].control_handle, &reply)
}

pub(super) fn handle_graph_status_request(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_index: usize,
    graph_status: GraphStatus,
    service_count: usize,
) -> rt::Result<()> {
    let mut reply = RawMessage::empty(ManagerTag::ServiceGraphStatusReply as u32);
    reply.word_count = 4;
    reply.words[0] = u64::from(graph_status.degraded_boot);
    reply.words[1] = graph_status.blocked_services as u64;
    reply.words[2] = graph_status.degraded_services as u64;
    reply.words[3] = service_count as u64;
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
    if slots[target_index].restart_requested || slots[target_index].phase == ServicePhase::Starting {
        reply.words[0] = ManagerStatus::Busy as u32 as u64;
        return rt::channel_send(slots[service_index].control_handle, &reply);
    }

    reply.words[0] = ManagerStatus::Ok as u32 as u64;
    rt::channel_send(slots[service_index].control_handle, &reply)?;
    if slots[target_index].control_handle != rt::INVALID_HANDLE {
        send_lifecycle(slots[target_index].control_handle, LifecycleEvent::Restarting)?;
    }
    slots[target_index].restart_requested = true;
    slots[target_index].next_restart_tick = 0;
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
