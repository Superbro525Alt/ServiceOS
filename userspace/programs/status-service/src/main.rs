#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

mod rollup;

use rollup::{RollupEntry, compute_rollup, fill_snapshot_reply, is_restarting_phase};
use rt::{
    ConfigKey, ControlTag, LifecycleEvent, LogDomain, LogEvent, LogSeverity, ManagerServicePhase,
    RawMessage, ServiceId, StatusHealth, StatusResult, StatusTag,
};
use serviceos_userspace_runtime as rt;

const MAX_BANNER_BYTES: usize = 128;
const MAX_STATUS_SERVICES: usize = 24;
const MAX_SUBSCRIBERS: usize = 8;

#[derive(Clone, Copy)]
struct ServiceStatusEntry {
    occupied: bool,
    service_id: ServiceId,
    phase: ManagerServicePhase,
    health: StatusHealth,
    detail_kind: u32,
    detail0: u64,
    detail1: u64,
    updated_tick: u64,
    restarts: u64,
}

impl ServiceStatusEntry {
    const fn empty() -> Self {
        Self {
            occupied: false,
            service_id: ServiceId::RootManager,
            phase: ManagerServicePhase::Dormant,
            health: StatusHealth::Unknown,
            detail_kind: rt::status_detail_kind::NONE,
            detail0: 0,
            detail1: 0,
            updated_tick: 0,
            restarts: 0,
        }
    }
}

#[derive(Clone, Copy)]
struct Subscriber {
    occupied: bool,
    handle: rt::Handle,
    filter: ServiceId,
}

impl Subscriber {
    const fn empty() -> Self {
        Self {
            occupied: false,
            handle: rt::INVALID_HANDLE,
            filter: ServiceId::RootManager,
        }
    }
}

#[cfg(not(test))]
rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf401;
    }
    if startup.handle_count < 2 || startup.word_count < 5 {
        return 0xf402;
    }

    let service_grants = startup.words[2] as usize;
    let resource_grants = startup.words[3] as usize;
    if service_grants < 1 || resource_grants < 1 {
        return 0xf403;
    }

    let log_handle = startup.handles[0];
    let banner_handle = startup.handles[service_grants];
    let banner_len = startup.words[4] as usize;
    let mut banner = [0u8; MAX_BANNER_BYTES];
    let requested = banner_len.min(banner.len());
    let banner_loaded = match rt::storage_read_all(banner_handle, &mut banner, requested) {
        Ok(loaded) => loaded,
        Err(_) => return 0xf404,
    };
    let _ = rt::storage_blob_close(banner_handle);
    if let Ok(text) = core::str::from_utf8(&banner[..banner_loaded]) {
        let _ = rt::write_logf("status", format_args!("resource: {}", text));
    }

    let config_handle = match rt::lookup_service(bootstrap, ServiceId::Config) {
        Ok(handle) => handle,
        Err(_) => return 0xf405,
    };
    let console_handle = match rt::lookup_service(bootstrap, ServiceId::Console) {
        Ok(handle) => handle,
        Err(_) => return 0xf406,
    };

    let heartbeat_ticks = match rt::config_read(config_handle, ConfigKey::StatusHeartbeatTicks) {
        Ok((_, value)) => value.max(1),
        Err(_) => return 0xf407,
    };
    let console_mirror = match rt::config_read(config_handle, ConfigKey::StatusConsoleMirror) {
        Ok((_, value)) => value,
        Err(_) => return 0xf408,
    };
    let heartbeat_log_period =
        match rt::config_read(config_handle, ConfigKey::StatusHeartbeatLogPeriod) {
            Ok((_, value)) => value,
            Err(_) => return 0xf409,
        };
    let _ = rt::handle_close(config_handle);

    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return 0xf40a,
    };
    if rt::register_service(bootstrap, ServiceId::Status, public.second).is_err() {
        return 0xf40b;
    }
    let _ = rt::handle_close(public.second);

    let mut entries = [ServiceStatusEntry::empty(); MAX_STATUS_SERVICES];
    let mut subscribers = [Subscriber::empty(); MAX_SUBSCRIBERS];
    let mut entry_count = 0usize;
    if seed_from_manager(bootstrap, &mut entries, &mut entry_count).is_err() {
        return 0xf40c;
    }

    let mut heartbeat_count = 0u64;
    let mut last_tick = 0u64;
    let mut next_heartbeat = match rt::monotonic_now() {
        Ok(now) => now.saturating_add(heartbeat_ticks),
        Err(_) => return 0xf40d,
    };

    let _ = rt::send_log_record(
        log_handle,
        ServiceId::Status,
        LogSeverity::Info,
        LogDomain::Status,
        LogEvent::StatusStarted,
        heartbeat_ticks,
        heartbeat_log_period,
    );
    let _ = rt::send_log_record(
        log_handle,
        ServiceId::Status,
        LogSeverity::Info,
        LogDomain::Status,
        LogEvent::ResourceOpened,
        banner_loaded as u64,
        0,
    );

    loop {
        let mut did_work = false;
        match poll_lifecycle(bootstrap) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(_) => return 0xf40e,
        }

        let mut request = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut request) {
            Ok(()) => {
                did_work = true;
                if handle_request(
                    &request,
                    &mut entries,
                    &mut entry_count,
                    &mut subscribers,
                    heartbeat_count,
                    last_tick,
                )
                .is_err()
                {
                    return 0xf40f;
                }
            }
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => return 0xf410,
        }

        let now = match rt::monotonic_now() {
            Ok(now) => now,
            Err(_) => return 0xf411,
        };
        if now >= next_heartbeat {
            did_work = true;
            heartbeat_count = heartbeat_count.saturating_add(1);
            last_tick = now;
            next_heartbeat = now.saturating_add(heartbeat_ticks);

            update_entry(
                &mut entries,
                &mut entry_count,
                &mut subscribers,
                ServiceId::Status,
                ManagerServicePhase::Ready,
                StatusHealth::Healthy,
                rt::status_detail_kind::HEARTBEAT,
                heartbeat_count,
                last_tick,
                now,
            );

            if heartbeat_log_period != 0 && heartbeat_count % heartbeat_log_period == 0 {
                let _ = rt::send_log_record(
                    log_handle,
                    ServiceId::Status,
                    LogSeverity::Info,
                    LogDomain::Status,
                    LogEvent::StatusHeartbeat,
                    heartbeat_count,
                    last_tick,
                );
            }
            if console_mirror != 0 && heartbeat_count % console_mirror == 0 {
                let _ = rt::console_write_record(
                    console_handle,
                    ServiceId::Status,
                    LogSeverity::Info,
                    LogDomain::Status,
                    LogEvent::ConsoleWrite,
                    heartbeat_count,
                    last_tick,
                    0,
                );
            }
        }

        if !did_work && rt::yield_current().is_err() {
            return 0xf412;
        }
    }
}

fn handle_request(
    request: &RawMessage,
    entries: &mut [ServiceStatusEntry; MAX_STATUS_SERVICES],
    entry_count: &mut usize,
    subscribers: &mut [Subscriber; MAX_SUBSCRIBERS],
    heartbeat_count: u64,
    last_tick: u64,
) -> rt::Result<()> {
    match request.tag {
        x if x == StatusTag::SnapshotRequest as u32 => {
            if request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut rollup_entries = [RollupEntry {
                service_id: 0,
                health: StatusHealth::Unknown,
                phase: ManagerServicePhase::Dormant,
                restarts: 0,
            }; MAX_STATUS_SERVICES];
            let mut rollup_count = 0usize;
            for entry in entries[..*entry_count]
                .iter()
                .filter(|entry| entry.occupied)
            {
                rollup_entries[rollup_count] = RollupEntry {
                    service_id: entry.service_id as u32,
                    health: entry.health,
                    phase: entry.phase,
                    restarts: entry.restarts,
                };
                rollup_count += 1;
            }
            let summary = compute_rollup(&rollup_entries[..rollup_count]);
            let mut reply = RawMessage::empty(StatusTag::SnapshotReply as u32);
            fill_snapshot_reply(&mut reply, heartbeat_count, last_tick, &summary);
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == StatusTag::ServiceReport as u32 => {
            if request.word_count < 7 {
                return Ok(());
            }
            update_entry(
                entries,
                entry_count,
                subscribers,
                service_id_from_word(request.words[0]),
                manager_phase_from_word(request.words[1]),
                health_from_word(request.words[2]),
                request.words[3] as u32,
                request.words[4],
                request.words[5],
                request.words[6],
            );
        }
        x if x == StatusTag::ServiceQueryRequest as u32 => {
            if request.word_count < 1 || request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let requested = service_id_from_word(request.words[0]);
            let mut reply = RawMessage::empty(StatusTag::ServiceQueryReply as u32);
            if let Some(entry) = find_entry(entries, *entry_count, requested) {
                reply.word_count = 8;
                reply.words[0] = StatusResult::Ok as u32 as u64;
                reply.words[1] = entry.service_id as u32 as u64;
                reply.words[2] = entry.phase as u32 as u64;
                reply.words[3] = entry.health as u32 as u64;
                reply.words[4] = entry.detail_kind as u64;
                reply.words[5] = entry.detail0;
                reply.words[6] = entry.detail1;
                reply.words[7] = entry.updated_tick;
            } else {
                reply.word_count = 1;
                reply.words[0] = StatusResult::NotFound as u32 as u64;
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == StatusTag::ServiceListRequest as u32 => {
            if request.handle_count < 1 {
                return Ok(());
            }
            let page_start = if request.word_count > 0 {
                request.words[0] as usize
            } else {
                0
            };
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(StatusTag::ServiceListReply as u32);
            reply.word_count = 2;
            reply.words[0] = 0;
            reply.words[1] = u64::MAX;
            let mut visible_index = 0usize;
            let mut emitted = 0usize;
            for entry in entries[..*entry_count]
                .iter()
                .copied()
                .filter(|entry| entry.occupied)
            {
                if visible_index < page_start {
                    visible_index += 1;
                    continue;
                }
                if reply.word_count as usize + 7 > rt::IPC_MAX_WORDS {
                    reply.words[1] = visible_index as u64;
                    break;
                }
                let base = reply.word_count as usize;
                reply.words[base] = entry.service_id as u32 as u64;
                reply.words[base + 1] = entry.phase as u32 as u64;
                reply.words[base + 2] = entry.health as u32 as u64;
                reply.words[base + 3] = entry.detail_kind as u64;
                reply.words[base + 4] = entry.detail0;
                reply.words[base + 5] = entry.detail1;
                reply.words[base + 6] = entry.updated_tick;
                reply.word_count += 7;
                emitted += 1;
                visible_index += 1;
            }
            reply.words[0] = emitted as u64;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == StatusTag::SubscribeRequest as u32 => {
            if request.handle_count < 1 {
                return Ok(());
            }
            let subscription_handle = request.handles[0];
            let mut reply = RawMessage::empty(StatusTag::SubscribeReply as u32);
            reply.word_count = 1;
            if let Some(index) = subscribers.iter().position(|entry| !entry.occupied) {
                subscribers[index] = Subscriber {
                    occupied: true,
                    handle: subscription_handle,
                    filter: if request.word_count > 0 {
                        service_id_from_word(request.words[0])
                    } else {
                        ServiceId::RootManager
                    },
                };
                reply.words[0] = StatusResult::Ok as u32 as u64;
            } else {
                reply.words[0] = StatusResult::Busy as u32 as u64;
                let _ = rt::handle_close(subscription_handle);
            }
            if let Some(reply_handle) = request.handles.get(1).copied() {
                if reply_handle != rt::INVALID_HANDLE {
                    let _ = rt::channel_send(reply_handle, &reply);
                    let _ = rt::handle_close(reply_handle);
                }
            }
        }
        _ => {}
    }

    Ok(())
}

fn seed_from_manager(
    bootstrap: rt::Handle,
    entries: &mut [ServiceStatusEntry; MAX_STATUS_SERVICES],
    entry_count: &mut usize,
) -> rt::Result<()> {
    let mut services = [rt::ManagerServiceInfo {
        service_id: ServiceId::RootManager,
        phase: ManagerServicePhase::Dormant,
        attempts: 0,
    }; MAX_STATUS_SERVICES];
    let count = rt::manager_list_services(bootstrap, &mut services)?;
    let mut no_subscribers = [Subscriber::empty(); 0];
    for service in services[..count].iter().copied() {
        let info = rt::manager_service_status(bootstrap, service.service_id)?;
        let (detail_kind, detail0, detail1) = manager_detail(
            info.phase,
            info.blocked_dependency,
            info.next_restart_tick,
            info.last_exit,
        );
        update_entry(
            entries,
            entry_count,
            &mut no_subscribers,
            service.service_id,
            info.phase,
            health_for_phase(info.phase),
            detail_kind,
            detail0,
            detail1,
            info.last_ready_tick.max(info.last_start_tick),
        );
    }
    Ok(())
}

fn update_entry(
    entries: &mut [ServiceStatusEntry; MAX_STATUS_SERVICES],
    entry_count: &mut usize,
    subscribers: &mut [Subscriber],
    service_id: ServiceId,
    phase: ManagerServicePhase,
    health: StatusHealth,
    detail_kind: u32,
    detail0: u64,
    detail1: u64,
    updated_tick: u64,
) {
    let (index, prior) = match entries[..*entry_count]
        .iter()
        .position(|entry| entry.occupied && entry.service_id == service_id)
    {
        Some(index) => (index, Some(entries[index])),
        None if *entry_count < entries.len() => {
            let index = *entry_count;
            *entry_count += 1;
            (index, None)
        }
        None => return,
    };

    let prior_restarts = prior.map_or(0, |entry| entry.restarts);
    let was_restarting = prior.is_some_and(|entry| is_restarting_phase(entry.phase));
    let restarts = if detail_kind == rt::status_detail_kind::RESTART_BACKOFF && !was_restarting {
        prior_restarts.saturating_add(1)
    } else {
        prior_restarts
    };

    entries[index] = ServiceStatusEntry {
        occupied: true,
        service_id,
        phase,
        health,
        detail_kind,
        detail0,
        detail1,
        updated_tick,
        restarts,
    };

    let mut event = RawMessage::empty(StatusTag::StreamEvent as u32);
    event.word_count = 7;
    event.words[0] = service_id as u32 as u64;
    event.words[1] = phase as u32 as u64;
    event.words[2] = health as u32 as u64;
    event.words[3] = detail_kind as u64;
    event.words[4] = detail0;
    event.words[5] = detail1;
    event.words[6] = updated_tick;

    for subscriber in subscribers.iter_mut().filter(|entry| entry.occupied) {
        if subscriber.filter != ServiceId::RootManager && subscriber.filter != service_id {
            continue;
        }
        if rt::channel_send(subscriber.handle, &event).is_err() {
            let _ = rt::handle_close(subscriber.handle);
            *subscriber = Subscriber::empty();
        }
    }
}

fn find_entry(
    entries: &[ServiceStatusEntry; MAX_STATUS_SERVICES],
    entry_count: usize,
    service_id: ServiceId,
) -> Option<ServiceStatusEntry> {
    entries[..entry_count]
        .iter()
        .copied()
        .find(|entry| entry.occupied && entry.service_id == service_id)
}

fn manager_detail(
    phase: ManagerServicePhase,
    blocked_dependency: ServiceId,
    next_restart_tick: u64,
    last_exit: u64,
) -> (u32, u64, u64) {
    match phase {
        ManagerServicePhase::WaitingDependencies => (
            rt::status_detail_kind::BLOCKED_DEPENDENCY,
            blocked_dependency as u32 as u64,
            0,
        ),
        ManagerServicePhase::Backoff => (
            rt::status_detail_kind::RESTART_BACKOFF,
            next_restart_tick,
            0,
        ),
        ManagerServicePhase::Degraded | ManagerServicePhase::Exited => {
            (rt::status_detail_kind::LIFECYCLE, last_exit, 0)
        }
        _ => (rt::status_detail_kind::LIFECYCLE, 0, 0),
    }
}

fn health_for_phase(phase: ManagerServicePhase) -> StatusHealth {
    match phase {
        ManagerServicePhase::Ready => StatusHealth::Healthy,
        ManagerServicePhase::Backoff => StatusHealth::Recovering,
        ManagerServicePhase::Degraded => StatusHealth::Degraded,
        ManagerServicePhase::Exited => StatusHealth::Failing,
        ManagerServicePhase::Dormant => StatusHealth::Dormant,
        ManagerServicePhase::WaitingDependencies | ManagerServicePhase::Starting => {
            StatusHealth::Recovering
        }
    }
}

fn poll_lifecycle(bootstrap: rt::Handle) -> rt::Result<bool> {
    let mut message = RawMessage::empty(0);
    match rt::channel_receive_nonblocking(bootstrap, &mut message) {
        Ok(()) if message.tag == ControlTag::Lifecycle as u32 && message.word_count > 0 => {
            Ok(matches!(
                lifecycle_event_from_word(message.words[0]),
                LifecycleEvent::Restarting | LifecycleEvent::Stopped
            ))
        }
        Ok(()) => Ok(false),
        Err(rt::Error::QueueEmpty) => Ok(false),
        Err(error) => Err(error),
    }
}

fn lifecycle_event_from_word(value: u64) -> LifecycleEvent {
    match value as u32 {
        x if x == LifecycleEvent::Starting as u32 => LifecycleEvent::Starting,
        x if x == LifecycleEvent::Ready as u32 => LifecycleEvent::Ready,
        x if x == LifecycleEvent::Failed as u32 => LifecycleEvent::Failed,
        x if x == LifecycleEvent::Stopped as u32 => LifecycleEvent::Stopped,
        _ => LifecycleEvent::Restarting,
    }
}

fn service_id_from_word(value: u64) -> ServiceId {
    match value as u32 {
        x if x == ServiceId::Storage as u32 => ServiceId::Storage,
        x if x == ServiceId::Console as u32 => ServiceId::Console,
        x if x == ServiceId::Config as u32 => ServiceId::Config,
        x if x == ServiceId::Log as u32 => ServiceId::Log,
        x if x == ServiceId::Status as u32 => ServiceId::Status,
        x if x == ServiceId::Shell as u32 => ServiceId::Shell,
        x if x == ServiceId::Package as u32 => ServiceId::Package,
        x if x == ServiceId::Announce as u32 => ServiceId::Announce,
        x if x == ServiceId::Network as u32 => ServiceId::Network,
        x if x == ServiceId::Graphics as u32 => ServiceId::Graphics,
        x if x == ServiceId::Session as u32 => ServiceId::Session,
        x if x == ServiceId::DesktopShell as u32 => ServiceId::DesktopShell,
        x if x == ServiceId::Terminal as u32 => ServiceId::Terminal,
        x if x == ServiceId::Audio as u32 => ServiceId::Audio,
        x if x == ServiceId::Runtime as u32 => ServiceId::Runtime,
        x if x == ServiceId::Developer as u32 => ServiceId::Developer,
        x if x == ServiceId::Clipboard as u32 => ServiceId::Clipboard,
        x if x == ServiceId::Security as u32 => ServiceId::Security,
        _ => ServiceId::RootManager,
    }
}

fn manager_phase_from_word(value: u64) -> ManagerServicePhase {
    match value as u32 {
        x if x == ManagerServicePhase::WaitingDependencies as u32 => {
            ManagerServicePhase::WaitingDependencies
        }
        x if x == ManagerServicePhase::Starting as u32 => ManagerServicePhase::Starting,
        x if x == ManagerServicePhase::Ready as u32 => ManagerServicePhase::Ready,
        x if x == ManagerServicePhase::Backoff as u32 => ManagerServicePhase::Backoff,
        x if x == ManagerServicePhase::Degraded as u32 => ManagerServicePhase::Degraded,
        x if x == ManagerServicePhase::Exited as u32 => ManagerServicePhase::Exited,
        _ => ManagerServicePhase::Dormant,
    }
}

fn health_from_word(value: u64) -> StatusHealth {
    match value as u32 {
        x if x == StatusHealth::Healthy as u32 => StatusHealth::Healthy,
        x if x == StatusHealth::Degraded as u32 => StatusHealth::Degraded,
        x if x == StatusHealth::Failing as u32 => StatusHealth::Failing,
        x if x == StatusHealth::Recovering as u32 => StatusHealth::Recovering,
        x if x == StatusHealth::Dormant as u32 => StatusHealth::Dormant,
        _ => StatusHealth::Unknown,
    }
}
