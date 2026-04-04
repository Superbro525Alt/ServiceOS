use serviceos_bundle::{BOOT_STORE_PATH_MAX, ServiceStartupMode};
use serviceos_userspace_runtime as rt;
use rt::{LogEvent, LogSeverity, ManagerStatus, ManagerTag, RawMessage, ServiceId};

use crate::{
    graph::{start_service, wait_until_ready},
    state::{BootstrapResources, ServiceSlot, MAX_SERVICE_SLOTS},
    util::{
        allocate_slot, compact_service_slots, emit_manager_event, find_slot_index_checked,
    },
};

use super::{
    lifecycle::{close_slot_for_failure, stop_service_slot},
    storage::{load_manifest_from_storage, unpack_path},
};

pub(super) fn handle_activate_request(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: &mut usize,
    service_index: usize,
    bootstrap_authority: rt::Handle,
    bootstrap_resources: BootstrapResources,
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
    let path = match unpack_path(&message.words[1..message.word_count as usize], path_len, &mut path_bytes) {
        Ok(path) => path,
        Err(_) => {
            reply.words[0] = ManagerStatus::Failed as u32 as u64;
            return rt::channel_send(control_handle, &reply);
        }
    };

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

    let target_index =
        if let Some(index) = find_slot_index_checked(slots, *service_count, manifest.service_id) {
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

    let result = if manifest.startup == ServiceStartupMode::OnDemand {
        slots[target_index].phase = crate::state::ServicePhase::Dormant;
        Ok(())
    } else {
        start_service(
            slots,
            *service_count,
            target_index,
            bootstrap_authority,
            bootstrap_resources,
        )
        .and_then(|_| {
            wait_until_ready(
                slots,
                service_count,
                bootstrap_authority,
                bootstrap_resources,
                manifest.service_id,
            )
        })
    };

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

pub(super) fn handle_deactivate_request(
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

    let requested = crate::util::service_id_from_word(requested_word);
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
