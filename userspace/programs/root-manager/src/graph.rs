use core::str;

use serviceos_userspace_runtime as rt;
use rt::{LogEvent, LogSeverity, ServiceId, TaskStateCode, rights, IPC_MAX_HANDLES};

use crate::control::{load_manifest_from_storage, pump_control_channels};
use crate::state::{
    BootstrapResources, ServicePhase, ServiceSlot, MAX_INDEX_BYTES, MAX_SERVICE_SLOTS,
};
use crate::util::{
    allocate_slot, bootstrap_resource_for, close_slot_handles, dependencies_ready,
    emit_manager_event, fallback_logf, find_slot_index, occupied_service_count,
    ready_service_count, service_name,
};

pub(crate) fn load_base_service_graph(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: &mut usize,
    index_path: &str,
) -> rt::Result<()> {
    let storage_index = find_slot_index(slots, *service_count, ServiceId::Storage)?;
    let storage_handle = slots[storage_index].public_handle;
    let (index_handle, index_len) = rt::storage_open(storage_handle, index_path)?;
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

pub(crate) fn activate_base_service_graph(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: &mut usize,
    bootstrap_authority: rt::Handle,
    bootstrap_resources: BootstrapResources,
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
                start_service(
                    slots,
                    *service_count,
                    index,
                    bootstrap_authority,
                    bootstrap_resource_for(slots[index].manifest.service_id, bootstrap_resources),
                )?;
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

pub(crate) fn start_service(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    index: usize,
    bootstrap_authority: rt::Handle,
    bootstrap_resource: Option<(rt::Handle, usize, u64)>,
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

    let mut startup = rt::RawMessage::empty(rt::ControlTag::Startup as u32);
    startup.word_count = 5;
    startup.words[0] = manifest.service_id as u32 as u64;
    startup.words[1] = slots[index].attempts as u64;
    startup.words[2] = manifest.grant_count as u64;
    startup.words[3] = manifest.resource_count as u64;
    startup.words[4] = bootstrap_resource.map(|(_, len, _)| len as u64).unwrap_or(0);

    let mut handle_index = 0usize;
    if let Some((handle, _, bootstrap_rights)) = bootstrap_resource {
        startup.handle_count = 1;
        startup.handles[0] = rt::handle_duplicate(
            handle,
            bootstrap_rights | rights::DUPLICATE | rights::TRANSFER,
        )?;
        startup.handle_rights[0] = bootstrap_rights;
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

pub(crate) fn wait_until_ready(
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
            let _ = crate::util::fallback_manager_event(
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

pub(crate) fn supervision_loop(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: &mut usize,
    bootstrap_authority: rt::Handle,
    bootstrap_resources: BootstrapResources,
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
                    bootstrap_resource_for(service_id, bootstrap_resources),
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
                serviceos_bundle::RestartPolicy::OnFailure { max_restarts } => max_restarts,
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
                bootstrap_resource_for(service_id, bootstrap_resources),
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
