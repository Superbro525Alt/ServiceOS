use serviceos_userspace_runtime as rt;
use rt::{ControlTag, ManagerTag, RawMessage};

use crate::state::{BootstrapResources, GraphStatus, ServiceSlot, MAX_SERVICE_SLOTS};

use super::{
    activation::{handle_activate_request, handle_deactivate_request},
    launch_requests::{
        handle_launch_image_request, handle_launch_request, handle_launch_stored_image_request,
    },
    lookup::{
        handle_graph_status_request, handle_list_services_request, handle_lookup_request,
        handle_service_lookup_list_request, handle_service_lookup_policy_set_request,
        handle_service_action_request, handle_service_status_request,
        handle_service_template_request,
    },
};

pub(crate) fn pump_control_channels(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: &mut usize,
    bootstrap_authority: rt::Handle,
    bootstrap_resources: BootstrapResources,
    graph_status: GraphStatus,
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
                handle_control_message(
                    slots,
                    service_count,
                    index,
                    bootstrap_authority,
                    bootstrap_resources,
                    graph_status,
                    &message,
                )?
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
    bootstrap_resources: BootstrapResources,
    graph_status: GraphStatus,
    message: &RawMessage,
) -> rt::Result<()> {
    match message.tag {
        x if x == ControlTag::Register as u32 => {
            if message.word_count < 1 || message.handle_count < 1 {
                return Err(rt::Error::InvalidArgument);
            }
            let service_id = crate::util::service_id_from_word(message.words[0]);
            if crate::util::find_slot_index(slots, *service_count, service_id)? != service_index {
                return Err(rt::Error::PermissionDenied);
            }
            slots[service_index].public_handle = message.handles[0];
            slots[service_index].phase = crate::state::ServicePhase::Ready;
            slots[service_index].consecutive_failures = 0;
            slots[service_index].blocked_dependency = rt::ServiceId::RootManager;
            slots[service_index].next_restart_tick = 0;
            slots[service_index].last_ready_tick = rt::monotonic_now().unwrap_or(0);
            let _ = crate::util::emit_manager_event(
                slots,
                *service_count,
                rt::LogSeverity::Info,
                rt::LogEvent::ServiceReady,
                service_id,
                slots[service_index].attempts as u64,
            );
        }
        x if x == ControlTag::LookupRequest as u32 => {
            handle_lookup_request(
                slots,
                service_count,
                service_index,
                bootstrap_authority,
                bootstrap_resources,
                message,
            )?
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
        x if x == ManagerTag::ServiceTemplateRequest as u32 => {
            if message.word_count < 1 {
                return Err(rt::Error::InvalidArgument);
            }
            handle_service_template_request(slots, *service_count, service_index, message.words[0])?;
        }
        x if x == ManagerTag::ServiceGraphStatusRequest as u32 => {
            handle_graph_status_request(slots, service_index, graph_status, *service_count)?;
        }
        x if x == ManagerTag::ServiceLookupListRequest as u32 => {
            if message.word_count < 2 {
                return Err(rt::Error::InvalidArgument);
            }
            handle_service_lookup_list_request(
                slots,
                *service_count,
                service_index,
                message.words[0],
                message.words[1] as usize,
            )?;
        }
        x if x == ManagerTag::ServiceLookupPolicySetRequest as u32 => {
            if message.word_count < 3 {
                return Err(rt::Error::InvalidArgument);
            }
            handle_service_lookup_policy_set_request(
                slots,
                *service_count,
                service_index,
                message.words[0],
                message.words[1],
                message.words[2],
            )?;
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
        x if x == ManagerTag::LaunchImageRequest as u32 => {
            handle_launch_image_request(
                slots,
                *service_count,
                service_index,
                bootstrap_authority,
                message,
            )?;
        }
        x if x == ManagerTag::LaunchStoredImageRequest as u32 => {
            handle_launch_stored_image_request(
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
                bootstrap_resources,
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
