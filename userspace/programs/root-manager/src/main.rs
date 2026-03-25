#![no_std]
#![no_main]

use serviceos_abi::{
    rights, ControlTag, LifecycleEvent, RawMessage, ServiceId, ServiceImageId,
    TaskStateCode, IPC_MAX_HANDLES,
};
use serviceos_userspace_runtime as rt;

const SERVICE_COUNT: usize = 3;

#[derive(Clone, Copy)]
enum RestartPolicy {
    Never,
    OnFailure { max_restarts: u32 },
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ServiceMode {
    LongRunning,
    OneShot,
}

#[derive(Clone, Copy)]
struct CapabilityGrant {
    source: ServiceId,
    rights: u64,
}

#[derive(Clone, Copy)]
struct ServiceManifest {
    id: ServiceId,
    name: &'static str,
    image: ServiceImageId,
    dependencies: &'static [ServiceId],
    mode: ServiceMode,
    grants: &'static [CapabilityGrant],
    restart: RestartPolicy,
}

const NO_DEPS: &[ServiceId] = &[];
const LOG_DEPS: &[ServiceId] = &[ServiceId::Log];
const LOG_ECHO_DEPS: &[ServiceId] = &[ServiceId::Log, ServiceId::Echo];
const NO_GRANTS: &[CapabilityGrant] = &[];
const LOG_SINK_GRANT: &[CapabilityGrant] = &[CapabilityGrant {
    source: ServiceId::Log,
    rights: rights::SEND | rights::DUPLICATE | rights::TRANSFER,
}];

const MANIFESTS: [ServiceManifest; SERVICE_COUNT] = [
    ServiceManifest {
        id: ServiceId::Log,
        name: "log-service",
        image: ServiceImageId::LogService,
        dependencies: NO_DEPS,
        mode: ServiceMode::LongRunning,
        grants: NO_GRANTS,
        restart: RestartPolicy::Never,
    },
    ServiceManifest {
        id: ServiceId::Echo,
        name: "echo-service",
        image: ServiceImageId::EchoService,
        dependencies: LOG_DEPS,
        mode: ServiceMode::LongRunning,
        grants: LOG_SINK_GRANT,
        restart: RestartPolicy::Never,
    },
    ServiceManifest {
        id: ServiceId::Probe,
        name: "probe-service",
        image: ServiceImageId::ProbeService,
        dependencies: LOG_ECHO_DEPS,
        mode: ServiceMode::OneShot,
        grants: LOG_SINK_GRANT,
        restart: RestartPolicy::OnFailure { max_restarts: 2 },
    },
];

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

rt::entry!(main);

fn main() -> u64 {
    let _ = rt::write_log("service-manager", "bootstrap started");
    let mut slots = [
        ServiceSlot::new(MANIFESTS[0]),
        ServiceSlot::new(MANIFESTS[1]),
        ServiceSlot::new(MANIFESTS[2]),
    ];

    for index in 0..SERVICE_COUNT {
        if activate_service(&mut slots, index).is_err() {
            let _ = rt::write_logf(
                "service-manager",
                format_args!("service {} activation failed", slots[index].manifest.name),
            );
            return 0xe002;
        }
    }

    let _ = rt::write_log("service-manager", "service graph initialized");
    let _ = rt::write_log("service-manager", "entering supervision loop");
    run_supervision_loop(&mut slots)
}

fn activate_service(slots: &mut [ServiceSlot; SERVICE_COUNT], index: usize) -> rt::Result<()> {
    start_service(slots, index)?;
    match slots[index].manifest.mode {
        ServiceMode::LongRunning => wait_until_ready(slots, slots[index].manifest.id),
        ServiceMode::OneShot => supervise_until_complete(slots, index),
    }
}

fn supervise_until_complete(slots: &mut [ServiceSlot; SERVICE_COUNT], service_index: usize) -> rt::Result<()> {
    loop {
        pump_control_channels(slots)?;
        let status = rt::task_status(slots[service_index].task_handle)?;
        match status.state {
            TaskStateCode::Running => rt::yield_current()?,
            TaskStateCode::Exited if status.exit_code == 0 => {
                let service_id = slots[service_index].manifest.id;
                slots[service_index].phase = ServicePhase::Exited;
                slots[service_index].last_exit_code = 0;
                emit_manager_lifecycle(slots, service_id, LifecycleEvent::Stopped, 0)?;
                return Ok(());
            }
            TaskStateCode::Exited => {
                let service_id = slots[service_index].manifest.id;
                slots[service_index].phase = ServicePhase::Exited;
                slots[service_index].last_exit_code = status.exit_code;
                emit_manager_lifecycle(slots, service_id, LifecycleEvent::Failed, status.exit_code)?;
                match slots[service_index].manifest.restart {
                    RestartPolicy::Never => return Err(rt::Error::Busy),
                    RestartPolicy::OnFailure { max_restarts } if slots[service_index].attempts < max_restarts => {
                        emit_manager_lifecycle(
                            slots,
                            service_id,
                            LifecycleEvent::Restarting,
                            slots[service_index].attempts as u64 + 1,
                        )?;
                        start_service(slots, service_index)?;
                    }
                    RestartPolicy::OnFailure { .. } => return Err(rt::Error::Busy),
                }
            }
        }
    }
}

fn run_supervision_loop(slots: &mut [ServiceSlot; SERVICE_COUNT]) -> u64 {
    loop {
        if pump_control_channels(slots).is_err() {
            return 0xe003;
        }

        for index in 0..SERVICE_COUNT {
            let manifest = slots[index].manifest;
            if manifest.mode != ServiceMode::LongRunning || slots[index].task_handle == rt::INVALID_HANDLE {
                continue;
            }

            let status = match rt::task_status(slots[index].task_handle) {
                Ok(status) => status,
                Err(_) => return 0xe004,
            };
            if status.state != TaskStateCode::Exited {
                continue;
            }

            if slots[index].phase != ServicePhase::Exited {
                slots[index].phase = ServicePhase::Exited;
                slots[index].last_exit_code = status.exit_code;
                let _ = emit_manager_lifecycle(
                    slots,
                    manifest.id,
                    LifecycleEvent::Failed,
                    status.exit_code,
                );
            }

            match manifest.restart {
                RestartPolicy::Never => {}
                RestartPolicy::OnFailure { max_restarts } if slots[index].attempts < max_restarts => {
                    let _ = emit_manager_lifecycle(
                        slots,
                        manifest.id,
                        LifecycleEvent::Restarting,
                        slots[index].attempts as u64 + 1,
                    );
                    if start_service(slots, index).is_err() {
                        return 0xe005;
                    }
                }
                RestartPolicy::OnFailure { .. } => {}
            }
        }

        if rt::yield_current().is_err() {
            return 0xe006;
        }
    }
}

fn wait_until_ready(slots: &mut [ServiceSlot; SERVICE_COUNT], service_id: ServiceId) -> rt::Result<()> {
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

fn start_service(slots: &mut [ServiceSlot; SERVICE_COUNT], index: usize) -> rt::Result<()> {
    let manifest = slots[index].manifest;
    for dependency in manifest.dependencies {
        if slots[index_for(*dependency)].phase != ServicePhase::Ready {
            return Err(rt::Error::Busy);
        }
    }

    let channels = rt::channel_create()?;
    let task_handle = rt::service_spawn(manifest.image, channels.second)?;
    slots[index].task_handle = task_handle;
    slots[index].control_handle = channels.first;
    slots[index].public_handle = rt::INVALID_HANDLE;
    slots[index].attempts += 1;
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
        let source_handle = slots[index_for(grant.source)].public_handle;
        let granted = rt::handle_duplicate(source_handle, grant.rights)?;
        startup.handle_count += 1;
        startup.handles[grant_index] = granted;
    }
    rt::channel_send(slots[index].control_handle, &startup)?;
    for handle in startup.handles[..startup.handle_count as usize].iter().copied() {
        let _ = rt::handle_close(handle);
    }

    let _ = rt::write_logf(
        "service-manager",
        format_args!("starting {} attempt={}", manifest.name, slots[index].attempts),
    );
    Ok(())
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
            let service_id = service_id_from_word(message.words[0]);
            let index = index_for(service_id);
            let attempts = slots[index].attempts;
            slots[index].public_handle = message.handles[0];
            slots[index].phase = ServicePhase::Ready;
            emit_manager_lifecycle(slots, service_id, LifecycleEvent::Ready, attempts as u64)?;
        }
        x if x == ControlTag::LookupRequest as u32 => {
            let requested = service_id_from_word(message.words[0]);
            let response = &slots[service_index];
            let target = &slots[index_for(requested)];
            let duplicated = rt::handle_duplicate(
                target.public_handle,
                rights::SEND | rights::RECEIVE | rights::DUPLICATE | rights::TRANSFER,
            )?;
            let mut reply = RawMessage::empty(ControlTag::LookupReply as u32);
            reply.word_count = 1;
            reply.words[0] = requested as u32 as u64;
            reply.handle_count = 1;
            reply.handles[0] = duplicated;
            rt::channel_send(response.control_handle, &reply)?;
            let _ = rt::handle_close(duplicated);
        }
        x if x == ControlTag::Lifecycle as u32 => {
            let service_id = service_id_from_word(message.words[0]);
            let event = lifecycle_from_word(message.words[1]);
            let detail = if message.word_count > 2 { message.words[2] } else { 0 };
            emit_manager_lifecycle(slots, service_id, event, detail)?;
        }
        _ => {}
    }

    Ok(())
}

fn emit_manager_lifecycle(
    slots: &[ServiceSlot; SERVICE_COUNT],
    service_id: ServiceId,
    event: LifecycleEvent,
    detail: u64,
) -> rt::Result<()> {
    if service_id == ServiceId::Log && slots[index_for(ServiceId::Log)].public_handle == rt::INVALID_HANDLE {
        return rt::write_logf(
            "service-manager",
            format_args!("{} {:?}", service_name(service_id), event as u32),
        );
    }
    let log_handle = slots[index_for(ServiceId::Log)].public_handle;
    if log_handle == rt::INVALID_HANDLE {
        return Ok(());
    }
    let mut message = RawMessage::empty(ControlTag::Lifecycle as u32);
    message.word_count = 3;
    message.words[0] = service_id as u32 as u64;
    message.words[1] = event as u32 as u64;
    message.words[2] = detail;
    rt::channel_send(log_handle, &message)
}

fn index_for(service_id: ServiceId) -> usize {
    match service_id {
        ServiceId::Log => 0,
        ServiceId::Echo => 1,
        ServiceId::Probe => 2,
        ServiceId::RootManager => 0,
    }
}

fn service_name(service_id: ServiceId) -> &'static str {
    match service_id {
        ServiceId::RootManager => "root-manager",
        ServiceId::Log => "log-service",
        ServiceId::Echo => "echo-service",
        ServiceId::Probe => "probe-service",
    }
}

fn service_id_from_word(value: u64) -> ServiceId {
    match value as u32 {
        x if x == ServiceId::Log as u32 => ServiceId::Log,
        x if x == ServiceId::Echo as u32 => ServiceId::Echo,
        x if x == ServiceId::Probe as u32 => ServiceId::Probe,
        _ => ServiceId::RootManager,
    }
}

fn lifecycle_from_word(value: u64) -> LifecycleEvent {
    match value as u32 {
        x if x == LifecycleEvent::Starting as u32 => LifecycleEvent::Starting,
        x if x == LifecycleEvent::Ready as u32 => LifecycleEvent::Ready,
        x if x == LifecycleEvent::Failed as u32 => LifecycleEvent::Failed,
        x if x == LifecycleEvent::Restarting as u32 => LifecycleEvent::Restarting,
        _ => LifecycleEvent::Stopped,
    }
}
