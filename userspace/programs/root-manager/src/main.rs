#![no_std]
#![no_main]

use serviceos_userspace_runtime as rt;
use rt::{
    rights, ControlTag, LogDomain, LogEvent, LogSeverity, LookupStatus, RawMessage,
    ServiceId, ServiceImageId, TaskStateCode, IPC_MAX_HANDLES,
};

const SERVICE_COUNT: usize = 4;

#[derive(Clone, Copy)]
enum RestartPolicy {
    OnFailure { max_restarts: u32 },
}

#[derive(Clone, Copy)]
struct CapabilityGrant {
    source: ServiceId,
    rights: u64,
}

#[derive(Clone, Copy)]
struct LookupGrant {
    target: ServiceId,
    rights: u64,
}

#[derive(Clone, Copy)]
struct ServiceManifest {
    id: ServiceId,
    name: &'static str,
    image: ServiceImageId,
    dependencies: &'static [ServiceId],
    grants: &'static [CapabilityGrant],
    lookups: &'static [LookupGrant],
    restart: RestartPolicy,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ServicePhase {
    Dormant,
    Starting,
    Ready,
    Exited,
}

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
    const fn new(manifest: ServiceManifest) -> Self {
        Self {
            manifest,
            task_handle: rt::INVALID_HANDLE,
            control_handle: rt::INVALID_HANDLE,
            public_handle: rt::INVALID_HANDLE,
            attempts: 0,
            phase: ServicePhase::Dormant,
            last_exit_code: 0,
        }
    }
}

const NO_DEPS: &[ServiceId] = &[];
const LOG_DEPS: &[ServiceId] = &[ServiceId::Console, ServiceId::Config];
const STATUS_DEPS: &[ServiceId] = &[ServiceId::Log, ServiceId::Config, ServiceId::Console];
const NO_GRANTS: &[CapabilityGrant] = &[];
const NO_LOOKUPS: &[LookupGrant] = &[];
const LOG_STARTUP_GRANTS: &[CapabilityGrant] = &[
    CapabilityGrant {
        source: ServiceId::Console,
        rights: rights::SEND,
    },
    CapabilityGrant {
        source: ServiceId::Config,
        rights: rights::SEND,
    },
];
const STATUS_STARTUP_GRANTS: &[CapabilityGrant] = &[CapabilityGrant {
    source: ServiceId::Log,
    rights: rights::SEND,
}];
const STATUS_LOOKUPS: &[LookupGrant] = &[
    LookupGrant {
        target: ServiceId::Config,
        rights: rights::SEND,
    },
    LookupGrant {
        target: ServiceId::Console,
        rights: rights::SEND,
    },
];

const MANIFESTS: [ServiceManifest; SERVICE_COUNT] = [
    ServiceManifest {
        id: ServiceId::Console,
        name: "console-service",
        image: ServiceImageId::ConsoleService,
        dependencies: NO_DEPS,
        grants: NO_GRANTS,
        lookups: NO_LOOKUPS,
        restart: RestartPolicy::OnFailure { max_restarts: 1 },
    },
    ServiceManifest {
        id: ServiceId::Config,
        name: "config-service",
        image: ServiceImageId::ConfigService,
        dependencies: NO_DEPS,
        grants: NO_GRANTS,
        lookups: NO_LOOKUPS,
        restart: RestartPolicy::OnFailure { max_restarts: 1 },
    },
    ServiceManifest {
        id: ServiceId::Log,
        name: "log-service",
        image: ServiceImageId::LogService,
        dependencies: LOG_DEPS,
        grants: LOG_STARTUP_GRANTS,
        lookups: NO_LOOKUPS,
        restart: RestartPolicy::OnFailure { max_restarts: 1 },
    },
    ServiceManifest {
        id: ServiceId::Status,
        name: "status-service",
        image: ServiceImageId::StatusService,
        dependencies: STATUS_DEPS,
        grants: STATUS_STARTUP_GRANTS,
        lookups: STATUS_LOOKUPS,
        restart: RestartPolicy::OnFailure { max_restarts: 2 },
    },
];

rt::entry!(main);

fn main() -> u64 {
    fallback_log("bootstrap started");
    let mut slots = [
        ServiceSlot::new(MANIFESTS[0]),
        ServiceSlot::new(MANIFESTS[1]),
        ServiceSlot::new(MANIFESTS[2]),
        ServiceSlot::new(MANIFESTS[3]),
    ];

    for index in 0..SERVICE_COUNT {
        if activate_service(&mut slots, index).is_err() {
            fallback_logf(format_args!(
                "activation failed for {}",
                slots[index].manifest.name
            ));
            return 0xe100 + index as u64;
        }
    }

    let _ = emit_manager_event(
        &slots,
        LogSeverity::Info,
        LogEvent::ServiceReady,
        ServiceId::RootManager,
        0,
    );
    supervision_loop(&mut slots)
}

fn activate_service(slots: &mut [ServiceSlot; SERVICE_COUNT], index: usize) -> rt::Result<()> {
    start_service(slots, index)?;
    wait_until_ready(slots, slots[index].manifest.id)
}

fn start_service(slots: &mut [ServiceSlot; SERVICE_COUNT], index: usize) -> rt::Result<()> {
    let manifest = slots[index].manifest;
    for dependency in manifest.dependencies {
        if slots[index_for(*dependency)].phase != ServicePhase::Ready {
            return Err(rt::Error::Busy);
        }
    }

    close_slot_handles(&mut slots[index]);

    let channels = rt::channel_create()?;
    let task_handle = rt::service_spawn(manifest.image, channels.second)?;
    slots[index].task_handle = task_handle;
    slots[index].control_handle = channels.first;
    slots[index].attempts = slots[index].attempts.saturating_add(1);
    slots[index].phase = ServicePhase::Starting;
    slots[index].last_exit_code = 0;

    let mut startup = RawMessage::empty(ControlTag::Startup as u32);
    startup.word_count = 2;
    startup.words[0] = manifest.id as u32 as u64;
    startup.words[1] = slots[index].attempts as u64;
    if manifest.grants.len() > IPC_MAX_HANDLES {
        return Err(rt::Error::BufferTooSmall);
    }

    for (grant_index, grant) in manifest.grants.iter().copied().enumerate() {
        let source = slots[index_for(grant.source)].public_handle;
        let transferred = rt::handle_duplicate(source, grant.rights | rights::DUPLICATE | rights::TRANSFER)?;
        startup.handle_count += 1;
        startup.handles[grant_index] = transferred;
        startup.handle_rights[grant_index] = grant.rights;
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
        LogSeverity::Info,
        LogEvent::ServiceStarted,
        manifest.id,
        slots[index].attempts as u64,
    );
    Ok(())
}

fn wait_until_ready(
    slots: &mut [ServiceSlot; SERVICE_COUNT],
    service_id: ServiceId,
) -> rt::Result<()> {
    loop {
        pump_control_channels(slots)?;
        let slot = &slots[index_for(service_id)];
        if slot.phase == ServicePhase::Ready {
            return Ok(());
        }
        let status = rt::task_status(slot.task_handle)?;
        if status.state == TaskStateCode::Exited {
            return Err(rt::Error::Busy);
        }
        rt::yield_current()?;
    }
}

fn supervision_loop(slots: &mut [ServiceSlot; SERVICE_COUNT]) -> u64 {
    loop {
        if pump_control_channels(slots).is_err() {
            return 0xe200;
        }

        for index in 0..SERVICE_COUNT {
            let status = match rt::task_status(slots[index].task_handle) {
                Ok(status) => status,
                Err(_) => return 0xe201 + index as u64,
            };
            if status.state != TaskStateCode::Exited {
                continue;
            }

            if slots[index].phase != ServicePhase::Exited {
                slots[index].phase = ServicePhase::Exited;
                slots[index].last_exit_code = status.exit_code;
                let _ = emit_manager_event(
                    slots,
                    LogSeverity::Error,
                    LogEvent::ServiceFailed,
                    slots[index].manifest.id,
                    status.exit_code,
                );
            }

            match slots[index].manifest.restart {
                RestartPolicy::OnFailure { max_restarts } if slots[index].attempts < max_restarts => {
                    let _ = emit_manager_event(
                        slots,
                        LogSeverity::Warn,
                        LogEvent::ServiceRestarting,
                        slots[index].manifest.id,
                        slots[index].attempts as u64 + 1,
                    );
                    if start_service(slots, index).is_err() {
                        return 0xe210 + index as u64;
                    }
                    if wait_until_ready(slots, slots[index].manifest.id).is_err() {
                        return 0xe220 + index as u64;
                    }
                }
                RestartPolicy::OnFailure { .. } => return 0xe230 + index as u64,
            }
        }

        if rt::yield_current().is_err() {
            return 0xe240;
        }
    }
}

fn pump_control_channels(slots: &mut [ServiceSlot; SERVICE_COUNT]) -> rt::Result<()> {
    for index in 0..SERVICE_COUNT {
        if slots[index].control_handle == rt::INVALID_HANDLE {
            continue;
        }

        let mut message = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(slots[index].control_handle, &mut message) {
            Ok(()) => handle_control_message(slots, index, &message)?,
            Err(rt::Error::QueueEmpty) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn handle_control_message(
    slots: &mut [ServiceSlot; SERVICE_COUNT],
    service_index: usize,
    message: &RawMessage,
) -> rt::Result<()> {
    match message.tag {
        x if x == ControlTag::Register as u32 => {
            if message.word_count < 1 || message.handle_count < 1 {
                return Err(rt::Error::InvalidArgument);
            }
            let service_id = service_id_from_word(message.words[0]);
            let index = index_for(service_id);
            if index != service_index {
                return Err(rt::Error::PermissionDenied);
            }
            slots[index].public_handle = message.handles[0];
            slots[index].phase = ServicePhase::Ready;
            let _ = emit_manager_event(
                slots,
                LogSeverity::Info,
                LogEvent::ServiceReady,
                service_id,
                slots[index].attempts as u64,
            );
        }
        x if x == ControlTag::LookupRequest as u32 => {
            if message.word_count < 1 {
                return Err(rt::Error::InvalidArgument);
            }
            let requested = service_id_from_word(message.words[0]);
            let permission = lookup_rights(slots[service_index].manifest, requested);
            let target = &slots[index_for(requested)];

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
                        LogSeverity::Debug,
                        LogEvent::LookupGranted,
                        slots[service_index].manifest.id,
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
    slots: &[ServiceSlot; SERVICE_COUNT],
    severity: LogSeverity,
    event: LogEvent,
    target: ServiceId,
    detail: u64,
) -> rt::Result<()> {
    let log_handle = slots[index_for(ServiceId::Log)].public_handle;
    if log_handle == rt::INVALID_HANDLE {
        return fallback_manager_event(severity, event, target, detail);
    }

    rt::send_log_record(
        log_handle,
        ServiceId::RootManager,
        severity,
        LogDomain::ServiceManager,
        event,
        target as u32 as u64,
        detail,
    )
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

fn lookup_rights(manifest: ServiceManifest, requested: ServiceId) -> Option<u64> {
    manifest
        .lookups
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

fn index_for(service_id: ServiceId) -> usize {
    match service_id {
        ServiceId::Console => 0,
        ServiceId::Config => 1,
        ServiceId::Log => 2,
        ServiceId::Status => 3,
        ServiceId::RootManager => 0,
    }
}

fn service_id_from_word(value: u64) -> ServiceId {
    match value as u32 {
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
    }
}

fn fallback_log(message: &str) {
    let _ = rt::write_log("service-manager", message);
}

fn fallback_logf(args: core::fmt::Arguments<'_>) {
    let _ = rt::write_logf("service-manager", args);
}
