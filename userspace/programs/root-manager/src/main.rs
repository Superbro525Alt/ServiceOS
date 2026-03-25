#![no_std]
#![no_main]

use serviceos_bundle::{parse_manifest, RestartPolicy, ServiceManifest};
use serviceos_userspace_runtime as rt;
use rt::{
    rights, ControlTag, LogDomain, LogEvent, LogSeverity, LookupStatus, RawMessage, ServiceId,
    ServiceImageId, TaskStateCode, IPC_MAX_HANDLES,
};

const MAX_SERVICE_SLOTS: usize = 5;
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
    if startup.tag != ControlTag::Startup as u32 || startup.handle_count < 1 || startup.word_count < 1 {
        return 0xf602;
    }
    let bootstore_handle = startup.handles[0];
    let bootstore_len = startup.words[0] as usize;

    fallback_log("bootstrap started");
    let mut slots = [ServiceSlot::empty(); MAX_SERVICE_SLOTS];
    slots[0].manifest = storage_manifest();

    if start_service(&mut slots, 0, Some((bootstore_handle, bootstore_len))).is_err() {
        return 0xf603;
    }
    if wait_until_ready(&mut slots, ServiceId::Storage).is_err() {
        return 0xf604;
    }

    let storage_handle = slots[find_slot_index(&slots, ServiceId::Storage)].public_handle;
    let service_count = match load_service_graph(&mut slots, storage_handle) {
        Ok(count) => count,
        Err(_) => return 0xf605,
    };

    if activate_service_graph(&mut slots, service_count).is_err() {
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

    supervision_loop(&mut slots, service_count, (bootstore_handle, bootstore_len))
}

fn storage_manifest() -> ServiceManifest {
    let mut manifest = ServiceManifest::empty();
    manifest.service_id = ServiceId::Storage;
    manifest.image_id = ServiceImageId::StorageService;
    manifest.restart = RestartPolicy::OnFailure { max_restarts: 1 };
    manifest
}

fn load_service_graph(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    storage_handle: rt::Handle,
) -> rt::Result<usize> {
    let mut index_buffer = [0u8; MAX_INDEX_BYTES];
    let (index_handle, index_len) = rt::storage_open(storage_handle, "services/index.txt")?;
    let index_len = index_len.min(index_buffer.len());
    let loaded = rt::storage_read_all(index_handle, &mut index_buffer, index_len)?;
    let _ = rt::handle_close(index_handle);
    let index_text = core::str::from_utf8(&index_buffer[..loaded]).map_err(|_| rt::Error::InvalidArgument)?;

    let mut count = 1usize;
    for line in index_text.lines().map(|line| line.trim()).filter(|line| !line.is_empty()) {
        if count == MAX_SERVICE_SLOTS {
            return Err(rt::Error::CapacityExceeded);
        }
        let mut manifest_buffer = [0u8; MAX_MANIFEST_BYTES];
        let (manifest_handle, manifest_len) = rt::storage_open(storage_handle, line)?;
        let manifest_len = manifest_len.min(manifest_buffer.len());
        let loaded = rt::storage_read_all(manifest_handle, &mut manifest_buffer, manifest_len)?;
        let _ = rt::handle_close(manifest_handle);
        let manifest =
            parse_manifest(&manifest_buffer[..loaded]).map_err(|_| rt::Error::InvalidArgument)?;
        slots[count].manifest = manifest;
        let _ = emit_manager_event(
            slots,
            count + 1,
            LogSeverity::Info,
            LogEvent::ManifestLoaded,
            manifest.service_id,
            loaded as u64,
        );
        count += 1;
    }

    Ok(count)
}

fn activate_service_graph(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
) -> rt::Result<()> {
    loop {
        let mut progress = false;
        let mut ready = 0usize;

        for index in 0..service_count {
            match slots[index].phase {
                ServicePhase::Ready => {
                    ready += 1;
                }
                ServicePhase::Dormant => {
                    if dependencies_ready(slots, service_count, index) {
                        start_service(slots, index, None)?;
                        wait_until_ready(slots, slots[index].manifest.service_id)?;
                        progress = true;
                    }
                }
                ServicePhase::Starting | ServicePhase::Exited => {}
            }
        }

        if ready == service_count {
            return Ok(());
        }
        if !progress {
            return Err(rt::Error::Busy);
        }
    }
}

fn start_service(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    index: usize,
    bootstrap_resource: Option<(rt::Handle, usize)>,
) -> rt::Result<()> {
    let manifest = slots[index].manifest;
    if bootstrap_resource.is_none() && !dependencies_ready(slots, MAX_SERVICE_SLOTS, index) {
        return Err(rt::Error::Busy);
    }

    close_slot_handles(&mut slots[index]);

    let channels = rt::channel_create()?;
    let task_handle = rt::service_spawn(manifest.image_id, channels.second)?;
    slots[index].task_handle = task_handle;
    slots[index].control_handle = channels.first;
    slots[index].attempts = slots[index].attempts.saturating_add(1);
    slots[index].phase = ServicePhase::Starting;
    slots[index].last_exit_code = 0;

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
        startup.handles[0] = rt::handle_duplicate(handle, rights::READ | rights::DUPLICATE | rights::TRANSFER)?;
        startup.handle_rights[0] = rights::READ | rights::DUPLICATE | rights::TRANSFER;
        handle_index = 1;
    }

    if handle_index + manifest.grant_count + manifest.resource_count > IPC_MAX_HANDLES {
        return Err(rt::Error::BufferTooSmall);
    }

    for grant in manifest.grants[..manifest.grant_count].iter().copied() {
        let source = slots[find_slot_index(slots, grant.target)].public_handle;
        let transferred = rt::handle_duplicate(source, grant.rights | rights::DUPLICATE | rights::TRANSFER)?;
        startup.handles[handle_index] = transferred;
        startup.handle_rights[handle_index] = grant.rights;
        startup.handle_count += 1;
        handle_index += 1;
    }

    let storage_index = find_slot_index(slots, ServiceId::Storage);
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
            MAX_SERVICE_SLOTS,
            LogSeverity::Info,
            LogEvent::ResourceOpened,
            manifest.service_id,
            resource_len as u64,
        );
    }

    rt::channel_send(slots[index].control_handle, &startup)?;
    for handle in startup.handles[..startup.handle_count as usize].iter().copied() {
        let _ = rt::handle_close(handle);
    }

    let _ = emit_manager_event(
        slots,
        MAX_SERVICE_SLOTS,
        LogSeverity::Info,
        LogEvent::ServiceStarted,
        manifest.service_id,
        slots[index].attempts as u64,
    );
    Ok(())
}

fn wait_until_ready(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_id: ServiceId,
) -> rt::Result<()> {
    loop {
        pump_control_channels(slots, MAX_SERVICE_SLOTS)?;
        let slot = &slots[find_slot_index(slots, service_id)];
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
    service_count: usize,
    bootstrap_resource: (rt::Handle, usize),
) -> u64 {
    loop {
        if pump_control_channels(slots, service_count).is_err() {
            return 0xf610;
        }

        for index in 0..service_count {
            let status = match rt::task_status(slots[index].task_handle) {
                Ok(status) => status,
                Err(_) => return 0xf611 + index as u64,
            };
            if status.state != TaskStateCode::Exited {
                continue;
            }

            if slots[index].phase != ServicePhase::Exited {
                slots[index].phase = ServicePhase::Exited;
                slots[index].last_exit_code = status.exit_code;
                let _ = emit_manager_event(
                    slots,
                    service_count,
                    LogSeverity::Error,
                    LogEvent::ServiceFailed,
                    slots[index].manifest.service_id,
                    status.exit_code,
                );
            }

            let restart_limit = match slots[index].manifest.restart {
                RestartPolicy::OnFailure { max_restarts } => max_restarts,
            };
            if slots[index].attempts >= restart_limit {
                return 0xf620 + index as u64;
            }

            let _ = emit_manager_event(
                slots,
                service_count,
                LogSeverity::Warn,
                LogEvent::ServiceRestarting,
                slots[index].manifest.service_id,
                slots[index].attempts as u64 + 1,
            );

            let resource = if slots[index].manifest.service_id == ServiceId::Storage {
                Some(bootstrap_resource)
            } else {
                None
            };
            if start_service(slots, index, resource).is_err() {
                return 0xf630 + index as u64;
            }
            if wait_until_ready(slots, slots[index].manifest.service_id).is_err() {
                return 0xf640 + index as u64;
            }
        }

        if rt::yield_current().is_err() {
            return 0xf650;
        }
    }
}

fn pump_control_channels(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
) -> rt::Result<()> {
    for index in 0..service_count {
        if slots[index].control_handle == rt::INVALID_HANDLE {
            continue;
        }

        let mut message = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(slots[index].control_handle, &mut message) {
            Ok(()) => handle_control_message(slots, service_count, index, &message)?,
            Err(rt::Error::QueueEmpty) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn handle_control_message(
    slots: &mut [ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    service_index: usize,
    message: &RawMessage,
) -> rt::Result<()> {
    match message.tag {
        x if x == ControlTag::Register as u32 => {
            if message.word_count < 1 || message.handle_count < 1 {
                return Err(rt::Error::InvalidArgument);
            }
            let service_id = service_id_from_word(message.words[0]);
            if find_slot_index(slots, service_id) != service_index {
                return Err(rt::Error::PermissionDenied);
            }
            slots[service_index].public_handle = message.handles[0];
            slots[service_index].phase = ServicePhase::Ready;
            let _ = emit_manager_event(
                slots,
                service_count,
                LogSeverity::Info,
                LogEvent::ServiceReady,
                service_id,
                slots[service_index].attempts as u64,
            );
        }
        x if x == ControlTag::LookupRequest as u32 => {
            if message.word_count < 1 {
                return Err(rt::Error::InvalidArgument);
            }
            let requested = service_id_from_word(message.words[0]);
            let permission = lookup_rights(slots[service_index].manifest, requested);
            let target = &slots[find_slot_index(slots, requested)];

            let mut reply = RawMessage::empty(ControlTag::LookupReply as u32);
            reply.word_count = 2;
            reply.words[0] = requested as u32 as u64;

            match permission {
                Some(rights)
                    if target.phase == ServicePhase::Ready
                        && target.public_handle != rt::INVALID_HANDLE =>
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
                Some(_) => {
                    reply.words[1] = LookupStatus::Unavailable as u32 as u64;
                    rt::channel_send(slots[service_index].control_handle, &reply)?;
                }
                None => {
                    reply.words[1] = LookupStatus::Denied as u32 as u64;
                    rt::channel_send(slots[service_index].control_handle, &reply)?;
                }
            }
        }
        _ => {}
    }

    Ok(())
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

fn find_slot_index(slots: &[ServiceSlot; MAX_SERVICE_SLOTS], service_id: ServiceId) -> usize {
    find_slot_index_checked(slots, MAX_SERVICE_SLOTS, service_id).unwrap_or(0)
}

fn find_slot_index_checked(
    slots: &[ServiceSlot; MAX_SERVICE_SLOTS],
    service_count: usize,
    service_id: ServiceId,
) -> Option<usize> {
    (0..service_count).find(|index| slots[*index].manifest.service_id == service_id)
}

fn service_id_from_word(value: u64) -> ServiceId {
    match value as u32 {
        x if x == ServiceId::Storage as u32 => ServiceId::Storage,
        x if x == ServiceId::Console as u32 => ServiceId::Console,
        x if x == ServiceId::Config as u32 => ServiceId::Config,
        x if x == ServiceId::Log as u32 => ServiceId::Log,
        x if x == ServiceId::Status as u32 => ServiceId::Status,
        _ => ServiceId::RootManager,
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
    }
}

fn fallback_log(message: &str) {
    let _ = rt::write_log("service-manager", message);
}
