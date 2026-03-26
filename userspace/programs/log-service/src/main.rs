#![no_std]
#![no_main]

use serviceos_userspace_runtime as rt;
use rt::{
    ConfigKey, ControlTag, LifecycleEvent, LogDomain, LogEvent, LogQueryStatus, LogSeverity,
    LogTag, RawMessage, ServiceId,
};

const MAX_LOG_RECORDS: usize = 64;

#[derive(Clone, Copy)]
struct StoredRecord {
    sequence: u64,
    source: ServiceId,
    severity: LogSeverity,
    domain: LogDomain,
    event: LogEvent,
    arg0: u64,
    arg1: u64,
}

impl StoredRecord {
    const fn empty() -> Self {
        Self {
            sequence: 0,
            source: ServiceId::RootManager,
            severity: LogSeverity::Info,
            domain: LogDomain::Bootstrap,
            event: LogEvent::ServiceStarted,
            arg0: 0,
            arg1: 0,
        }
    }
}

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf101;
    }
    if startup.handle_count < 2 {
        return 0xf102;
    }

    let console_handle = startup.handles[0];
    let config_handle = startup.handles[1];
    let minimum_severity = match rt::config_read(config_handle, ConfigKey::LogMinimumSeverity) {
        Ok((_, value)) => value,
        Err(_) => return 0xf103,
    };
    let _ = rt::handle_close(config_handle);

    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return 0xf104,
    };
    if rt::register_service(bootstrap, ServiceId::Log, public.second).is_err() {
        return 0xf105;
    }
    let _ = rt::handle_close(public.second);
    let _ = rt::console_write_record(
        console_handle,
        ServiceId::Log,
        LogSeverity::Info,
        LogDomain::Log,
        LogEvent::ConfigLoaded,
        minimum_severity,
        0,
        0,
    );

    let mut records = [StoredRecord::empty(); MAX_LOG_RECORDS];
    let mut record_count = 0usize;
    let mut next_slot = 0usize;
    let mut sequence = 0u64;

    loop {
        match poll_lifecycle(bootstrap) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(_) => return 0xf106,
        }

        let mut message = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut message) {
            Ok(()) => {
                if handle_request(
                    &message,
                    console_handle,
                    minimum_severity,
                    &mut records,
                    &mut record_count,
                    &mut next_slot,
                    &mut sequence,
                )
                .is_err()
                {
                    return 0xf107;
                }
            }
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => return 0xf108,
        }

        if rt::yield_current().is_err() {
            return 0xf109;
        }
    }
}

fn handle_request(
    message: &RawMessage,
    console_handle: rt::Handle,
    minimum_severity: u64,
    records: &mut [StoredRecord; MAX_LOG_RECORDS],
    record_count: &mut usize,
    next_slot: &mut usize,
    sequence: &mut u64,
) -> rt::Result<()> {
    match message.tag {
        x if x == LogTag::Record as u32 => {
            if message.word_count < 6 {
                return Ok(());
            }
            if message.words[1] < minimum_severity {
                return Ok(());
            }

            *sequence = sequence.saturating_add(1);
            let record = StoredRecord {
                sequence: *sequence,
                source: service_id_from_word(message.words[0]),
                severity: severity_from_word(message.words[1]),
                domain: domain_from_word(message.words[2]),
                event: event_from_word(message.words[3]),
                arg0: message.words[4],
                arg1: message.words[5],
            };
            records[*next_slot] = record;
            *next_slot = (*next_slot + 1) % records.len();
            *record_count = (*record_count + 1).min(records.len());

            let _ = rt::console_write_record(
                console_handle,
                record.source,
                record.severity,
                record.domain,
                record.event,
                record.arg0,
                record.arg1,
                record.sequence,
            );
        }
        x if x == LogTag::QueryInfoRequest as u32 => {
            if message.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = message.handles[0];
            let mut reply = RawMessage::empty(LogTag::QueryInfoReply as u32);
            reply.word_count = 2;
            reply.words[0] = oldest_sequence(*sequence, *record_count);
            reply.words[1] = if *record_count == 0 {
                0
            } else {
                sequence.saturating_add(1)
            };
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == LogTag::QueryRecordRequest as u32 => {
            if message.word_count < 1 || message.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = message.handles[0];
            let mut reply = RawMessage::empty(LogTag::QueryRecordReply as u32);
            reply.word_count = 2;
            reply.words[0] = LogQueryStatus::NotFound as u32 as u64;
            reply.words[1] = message.words[0];

            if let Some(record) = find_record(records, *record_count, message.words[0]) {
                reply.word_count = 8;
                reply.words[0] = LogQueryStatus::Ok as u32 as u64;
                reply.words[1] = record.sequence;
                reply.words[2] = record.source as u32 as u64;
                reply.words[3] = record.severity as u32 as u64;
                reply.words[4] = record.domain as u32 as u64;
                reply.words[5] = record.event as u32 as u64;
                reply.words[6] = record.arg0;
                reply.words[7] = record.arg1;
            }

            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        _ => {}
    }

    Ok(())
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

fn oldest_sequence(next_sequence: u64, record_count: usize) -> u64 {
    if record_count == 0 {
        0
    } else {
        next_sequence.saturating_sub(record_count as u64).saturating_add(1)
    }
}

fn find_record(
    records: &[StoredRecord; MAX_LOG_RECORDS],
    record_count: usize,
    sequence: u64,
) -> Option<StoredRecord> {
    records[..record_count]
        .iter()
        .copied()
        .find(|record| record.sequence == sequence)
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
        _ => ServiceId::RootManager,
    }
}

fn severity_from_word(value: u64) -> LogSeverity {
    match value as u32 {
        x if x == LogSeverity::Trace as u32 => LogSeverity::Trace,
        x if x == LogSeverity::Debug as u32 => LogSeverity::Debug,
        x if x == LogSeverity::Warn as u32 => LogSeverity::Warn,
        x if x == LogSeverity::Error as u32 => LogSeverity::Error,
        _ => LogSeverity::Info,
    }
}

fn domain_from_word(value: u64) -> LogDomain {
    match value as u32 {
        x if x == LogDomain::Bootstrap as u32 => LogDomain::Bootstrap,
        x if x == LogDomain::ServiceManager as u32 => LogDomain::ServiceManager,
        x if x == LogDomain::Storage as u32 => LogDomain::Storage,
        x if x == LogDomain::Log as u32 => LogDomain::Log,
        x if x == LogDomain::Config as u32 => LogDomain::Config,
        x if x == LogDomain::Console as u32 => LogDomain::Console,
        x if x == LogDomain::Status as u32 => LogDomain::Status,
        x if x == LogDomain::Ipc as u32 => LogDomain::Ipc,
        x if x == LogDomain::Shell as u32 => LogDomain::Shell,
        x if x == LogDomain::Package as u32 => LogDomain::Package,
        x if x == LogDomain::Network as u32 => LogDomain::Network,
        _ => LogDomain::Service,
    }
}

fn event_from_word(value: u64) -> LogEvent {
    match value as u32 {
        x if x == LogEvent::ServiceStarted as u32 => LogEvent::ServiceStarted,
        x if x == LogEvent::ServiceReady as u32 => LogEvent::ServiceReady,
        x if x == LogEvent::ServiceFailed as u32 => LogEvent::ServiceFailed,
        x if x == LogEvent::ServiceRestarting as u32 => LogEvent::ServiceRestarting,
        x if x == LogEvent::ConfigLoaded as u32 => LogEvent::ConfigLoaded,
        x if x == LogEvent::ConfigRead as u32 => LogEvent::ConfigRead,
        x if x == LogEvent::ConsoleWrite as u32 => LogEvent::ConsoleWrite,
        x if x == LogEvent::StatusStarted as u32 => LogEvent::StatusStarted,
        x if x == LogEvent::StatusHeartbeat as u32 => LogEvent::StatusHeartbeat,
        x if x == LogEvent::StorageMounted as u32 => LogEvent::StorageMounted,
        x if x == LogEvent::ManifestLoaded as u32 => LogEvent::ManifestLoaded,
        x if x == LogEvent::ResourceOpened as u32 => LogEvent::ResourceOpened,
        x if x == LogEvent::SessionOpened as u32 => LogEvent::SessionOpened,
        x if x == LogEvent::ShellCommand as u32 => LogEvent::ShellCommand,
        x if x == LogEvent::ToolLaunched as u32 => LogEvent::ToolLaunched,
        x if x == LogEvent::PackageCatalogLoaded as u32 => LogEvent::PackageCatalogLoaded,
        x if x == LogEvent::PackageInstalled as u32 => LogEvent::PackageInstalled,
        x if x == LogEvent::PackageUpdated as u32 => LogEvent::PackageUpdated,
        x if x == LogEvent::PackageRemoved as u32 => LogEvent::PackageRemoved,
        x if x == LogEvent::PackageRolledBack as u32 => LogEvent::PackageRolledBack,
        x if x == LogEvent::PackageActivationFailed as u32 => LogEvent::PackageActivationFailed,
        x if x == LogEvent::NetworkInterfaceReady as u32 => LogEvent::NetworkInterfaceReady,
        x if x == LogEvent::NetworkAddressConfigured as u32 => LogEvent::NetworkAddressConfigured,
        x if x == LogEvent::NetworkResolveCompleted as u32 => LogEvent::NetworkResolveCompleted,
        x if x == LogEvent::NetworkProbeCompleted as u32 => LogEvent::NetworkProbeCompleted,
        x if x == LogEvent::NetworkLinkChanged as u32 => LogEvent::NetworkLinkChanged,
        _ => LogEvent::LookupGranted,
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
