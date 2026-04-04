use core::str;

use serviceos_bundle::{RestartPolicy, ServiceStartupMode};
use serviceos_userspace_runtime as rt;
use rt::{LogEvent, LogSeverity, ServiceId, TaskStateCode, rights, IPC_MAX_HANDLES};

use crate::control::{load_manifest_from_storage, pump_control_channels};
use crate::state::{
    BootstrapResources, GraphStatus, ServicePhase, ServiceSlot, MAX_INDEX_BYTES,
    MAX_SERVICE_SLOTS,
};
use crate::util::{
    allocate_slot, bootstrap_resource_for, close_slot_handles, dependencies_ready,
    emit_manager_event, fallback_logf, find_slot_index, first_unready_dependency,
    occupied_service_count, publish_manager_status, ready_service_count, service_name,
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
    graph_status: &mut GraphStatus,
) -> rt::Result<()> {
    loop {
        let total = occupied_service_count(slots, *service_count);
        let ready = ready_service_count(slots, *service_count);
        if ready == total {
            graph_status.blocked_services = 0;
            graph_status.degraded_services = count_phase(slots, *service_count, ServicePhase::Degraded) as u32;
            return Ok(());
        }

        let mut progress = false;
        let mut pending_eager = false;
        let mut blocked = 0u32;
        for index in 0..*service_count {
            if !slots[index].occupied {
                continue;
            }

            match slots[index].phase {
                ServicePhase::Dormant | ServicePhase::WaitingDependencies => {}
                ServicePhase::Starting | ServicePhase::Ready | ServicePhase::Backoff => continue,
                ServicePhase::Degraded | ServicePhase::Exited => continue,
            }

            if slots[index].manifest.startup == ServiceStartupMode::OnDemand {
                continue;
            }
            pending_eager = true;

            if !dependencies_ready(slots, *service_count, index) {
                mark_waiting_dependency(slots, *service_count, index);
                blocked = blocked.saturating_add(1);
                continue;
            }

            let _ = fallback_logf(format_args!(
                "activating {}",
                service_name(slots[index].manifest.service_id)
            ));
            match start_service(
                slots,
                *service_count,
                index,
                bootstrap_authority,
                bootstrap_resources,
            )
            .and_then(|_| {
                wait_until_ready(
                    slots,
                    service_count,
                    bootstrap_authority,
                    bootstrap_resources,
                    slots[index].manifest.service_id,
                )
            }) {
                Ok(()) => progress = true,
                Err(_) => {
                    progress = true;
                    graph_status.degraded_boot = true;
                    mark_service_degraded(slots, *service_count, index);
                }
            }
        }

        graph_status.blocked_services = blocked;
        graph_status.degraded_services = count_phase(slots, *service_count, ServicePhase::Degraded) as u32;

        if !pending_eager {
            return Ok(());
        }
        if !progress {
            graph_status.degraded_boot = true;
            emit_blocked_graph_diagnostics(slots, *service_count);
            return Ok(());
        }
    }
}

pub(crate) fn start_service(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    index: usize,
    bootstrap_authority: rt::Handle,
    bootstrap_resources: BootstrapResources,
) -> rt::Result<()> {
    let manifest = slots[index].manifest;
    let bootstrap_resource = bootstrap_resource_for(manifest.service_id, bootstrap_resources);
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
    slots[index].blocked_dependency = ServiceId::RootManager;
    slots[index].restart_requested = false;
    slots[index].next_restart_tick = 0;
    slots[index].last_start_tick = rt::monotonic_now().unwrap_or(0);
    publish_manager_status(
        slots,
        service_count,
        manifest.service_id,
        ServicePhase::Starting,
        rt::status_detail_kind::LIFECYCLE,
        slots[index].attempts as u64,
        0,
    );

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
    if manifest.service_id == ServiceId::Storage {
        if let Some((handle, len, block_rights)) = bootstrap_resources
            .block
            .map(|resource| (resource.handle, resource.len, resource.rights))
        {
            startup.handles[handle_index] = rt::handle_duplicate(
                handle,
                block_rights | rights::DUPLICATE | rights::TRANSFER,
            )?;
            startup.handle_rights[handle_index] = block_rights;
            startup.handle_count += 1;
            handle_index += 1;
            startup.word_count = 6;
            startup.words[5] = len as u64;
        }
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
    bootstrap_resources: BootstrapResources,
    service_id: ServiceId,
) -> rt::Result<()> {
    loop {
        pump_control_channels(
            slots,
            service_count,
            bootstrap_authority,
            bootstrap_resources,
            GraphStatus::empty(),
        )?;
        let slot_index = find_slot_index(slots, *service_count, service_id)?;
        let slot = &slots[slot_index];
        if slot.phase == ServicePhase::Ready {
            return Ok(());
        }
        if matches!(slot.phase, ServicePhase::Backoff | ServicePhase::Degraded | ServicePhase::Exited) {
            return Err(rt::Error::Busy);
        }
        let status = rt::task_status(slot.task_handle)?;
        if matches!(status.state, TaskStateCode::Exited | TaskStateCode::Faulted) {
            let _ = crate::util::fallback_manager_event(
                LogSeverity::Error,
                LogEvent::ServiceFailed,
                service_id,
                status.exit_code,
            );
            return Err(rt::Error::Busy);
        }
        if slot.manifest.ready_timeout_ticks != 0 {
            let now = rt::monotonic_now()?;
            if now.saturating_sub(slot.last_start_tick) >= slot.manifest.ready_timeout_ticks as u64 {
                return Err(rt::Error::Busy);
            }
        }
        rt::yield_current()?;
    }
}

pub(crate) fn ensure_service_ready(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: &mut usize,
    bootstrap_authority: rt::Handle,
    bootstrap_resources: BootstrapResources,
    service_id: ServiceId,
) -> rt::Result<()> {
    let index = find_slot_index(slots, *service_count, service_id)?;
    match slots[index].phase {
        ServicePhase::Ready => return Ok(()),
        ServicePhase::Dormant | ServicePhase::WaitingDependencies => {}
        ServicePhase::Backoff => {
            if rt::monotonic_now()? < slots[index].next_restart_tick {
                return Err(rt::Error::Busy);
            }
        }
        ServicePhase::Starting => {
            return wait_until_ready(
                slots,
                service_count,
                bootstrap_authority,
                bootstrap_resources,
                service_id,
            )
        }
        ServicePhase::Degraded | ServicePhase::Exited => return Err(rt::Error::Busy),
    }

    if !dependencies_ready(slots, *service_count, index) {
        mark_waiting_dependency(slots, *service_count, index);
        return Err(rt::Error::Busy);
    }

    start_service(
        slots,
        *service_count,
        index,
        bootstrap_authority,
        bootstrap_resources,
    )?;
    wait_until_ready(
        slots,
        service_count,
        bootstrap_authority,
        bootstrap_resources,
        service_id,
    )
}

pub(crate) fn supervision_loop(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: &mut usize,
    bootstrap_authority: rt::Handle,
    bootstrap_resources: BootstrapResources,
    graph_status: &mut GraphStatus,
) -> u64 {
    loop {
        if pump_control_channels(
            slots,
            service_count,
            bootstrap_authority,
            bootstrap_resources,
            *graph_status,
        )
        .is_err()
        {
            return 0xf610;
        }

        let now = match rt::monotonic_now() {
            Ok(now) => now,
            Err(_) => return 0xf611,
        };

        for index in 0..*service_count {
            if !slots[index].occupied {
                continue;
            }

            if slots[index].phase == ServicePhase::Backoff && now >= slots[index].next_restart_tick {
                if dependencies_ready(slots, *service_count, index) {
                    let service_id = slots[index].manifest.service_id;
                    let _ = fallback_logf(format_args!(
                        "restarting {} after backoff",
                        service_name(service_id)
                    ));
                    if start_service(
                        slots,
                        *service_count,
                        index,
                        bootstrap_authority,
                        bootstrap_resources,
                    )
                    .is_err()
                    {
                        mark_service_degraded(slots, *service_count, index);
                        graph_status.degraded_boot = true;
                    } else if wait_until_ready(
                        slots,
                        service_count,
                        bootstrap_authority,
                        bootstrap_resources,
                        service_id,
                    )
                    .is_err()
                    {
                        mark_service_degraded(slots, *service_count, index);
                        graph_status.degraded_boot = true;
                    }
                } else {
                    mark_waiting_dependency(slots, *service_count, index);
                }
            } else if slots[index].phase == ServicePhase::WaitingDependencies
                && slots[index].manifest.startup == ServiceStartupMode::Eager
                && dependencies_ready(slots, *service_count, index)
            {
                let service_id = slots[index].manifest.service_id;
                if start_service(
                    slots,
                    *service_count,
                    index,
                    bootstrap_authority,
                    bootstrap_resources,
                )
                .is_err()
                {
                    mark_service_degraded(slots, *service_count, index);
                    graph_status.degraded_boot = true;
                } else if wait_until_ready(
                    slots,
                    service_count,
                    bootstrap_authority,
                    bootstrap_resources,
                    service_id,
                )
                .is_err()
                {
                    mark_service_degraded(slots, *service_count, index);
                    graph_status.degraded_boot = true;
                }
            }
        }

        for index in 0..*service_count {
            if !slots[index].occupied || slots[index].task_handle == rt::INVALID_HANDLE {
                continue;
            }

            let status = match rt::task_status(slots[index].task_handle) {
                Ok(status) => status,
                Err(_) => return 0xf620 + index as u64,
            };
            if !matches!(status.state, TaskStateCode::Exited | TaskStateCode::Faulted) {
                continue;
            }

            let service_id = slots[index].manifest.service_id;
            let requested_restart = slots[index].restart_requested;
            slots[index].last_exit_code = status.exit_code;
            close_slot_handles(&mut slots[index]);

            if status.exit_code == 0 && slots[index].manifest.startup == ServiceStartupMode::OnDemand {
                slots[index].phase = ServicePhase::Dormant;
                slots[index].restart_requested = false;
                publish_manager_status(
                    slots,
                    *service_count,
                    service_id,
                    ServicePhase::Dormant,
                    rt::status_detail_kind::LIFECYCLE,
                    0,
                    0,
                );
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

            let (restart_limit, base_backoff) = match slots[index].manifest.restart {
                RestartPolicy::OnFailure {
                    max_restarts,
                    backoff_ticks,
                } => (max_restarts, backoff_ticks),
            };
            if status.exit_code == 0 {
                slots[index].phase = ServicePhase::Exited;
                slots[index].restart_requested = false;
                publish_manager_status(
                    slots,
                    *service_count,
                    service_id,
                    ServicePhase::Exited,
                    rt::status_detail_kind::LIFECYCLE,
                    status.exit_code,
                    0,
                );
                continue;
            }

            slots[index].consecutive_failures = slots[index].consecutive_failures.saturating_add(1);
            let failures = slots[index].attempts.saturating_sub(1);
            if !requested_restart && failures >= restart_limit {
                mark_service_degraded(slots, *service_count, index);
                graph_status.degraded_boot = true;
                continue;
            }

            let mut delay = base_backoff as u64;
            if delay != 0 && slots[index].consecutive_failures > 1 {
                let shift = (slots[index].consecutive_failures - 1).min(6);
                delay = delay.saturating_mul(1u64 << shift);
            }
            slots[index].phase = if delay == 0 {
                ServicePhase::Dormant
            } else {
                ServicePhase::Backoff
            };
            slots[index].next_restart_tick = now.saturating_add(delay);
            slots[index].restart_requested = false;
            publish_manager_status(
                slots,
                *service_count,
                service_id,
                slots[index].phase,
                if delay == 0 {
                    rt::status_detail_kind::LIFECYCLE
                } else {
                    rt::status_detail_kind::RESTART_BACKOFF
                },
                if delay == 0 {
                    status.exit_code
                } else {
                    slots[index].next_restart_tick
                },
                slots[index].consecutive_failures as u64,
            );

            let _ = emit_manager_event(
                slots,
                *service_count,
                LogSeverity::Warn,
                LogEvent::ServiceRestarting,
                service_id,
                slots[index].next_restart_tick,
            );

            if delay == 0 {
                if start_service(
                    slots,
                    *service_count,
                    index,
                    bootstrap_authority,
                    bootstrap_resources,
                )
                .is_err()
                {
                    mark_service_degraded(slots, *service_count, index);
                    graph_status.degraded_boot = true;
                } else if wait_until_ready(
                    slots,
                    service_count,
                    bootstrap_authority,
                    bootstrap_resources,
                    service_id,
                )
                .is_err()
                {
                    mark_service_degraded(slots, *service_count, index);
                    graph_status.degraded_boot = true;
                }
            }
        }

        graph_status.blocked_services = count_phase(slots, *service_count, ServicePhase::WaitingDependencies) as u32;
        graph_status.degraded_services = count_phase(slots, *service_count, ServicePhase::Degraded) as u32;

        if rt::yield_current().is_err() {
            return 0xf670;
        }
    }
}

fn count_phase(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    phase: ServicePhase,
) -> usize {
    slots[..service_count]
        .iter()
        .filter(|slot| slot.occupied && slot.phase == phase)
        .count()
}

fn mark_waiting_dependency(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    index: usize,
) {
    slots[index].phase = ServicePhase::WaitingDependencies;
    slots[index].blocked_dependency =
        first_unready_dependency(slots, service_count, index).unwrap_or(ServiceId::RootManager);
    publish_manager_status(
        slots,
        service_count,
        slots[index].manifest.service_id,
        ServicePhase::WaitingDependencies,
        rt::status_detail_kind::BLOCKED_DEPENDENCY,
        slots[index].blocked_dependency as u32 as u64,
        0,
    );
}

fn mark_service_degraded(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    index: usize,
) {
    slots[index].phase = ServicePhase::Degraded;
    slots[index].restart_requested = false;
    slots[index].next_restart_tick = 0;
    publish_manager_status(
        slots,
        service_count,
        slots[index].manifest.service_id,
        ServicePhase::Degraded,
        rt::status_detail_kind::LIFECYCLE,
        slots[index].last_exit_code,
        slots[index].consecutive_failures as u64,
    );
}

fn emit_blocked_graph_diagnostics(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
) {
    for slot in &slots[..service_count] {
        if !slot.occupied || slot.phase != ServicePhase::WaitingDependencies {
            continue;
        }
        let _ = fallback_logf(format_args!(
            "service {} blocked waiting for {}",
            service_name(slot.manifest.service_id),
            service_name(slot.blocked_dependency),
        ));
    }
}
