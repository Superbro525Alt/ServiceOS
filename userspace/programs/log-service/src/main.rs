#![no_std]
#![no_main]

use serviceos_userspace_runtime as rt;
use rt::{
    ConfigKey, LogDomain, LogEvent, LogSeverity, LogTag, RawMessage, ServiceId,
};

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

    let mut sequence = 0u64;
    loop {
        let mut message = RawMessage::empty(0);
        if rt::channel_receive_blocking(public.first, &mut message).is_err() {
            return 0xf106;
        }
        if message.tag != LogTag::Record as u32 || message.word_count < 6 {
            continue;
        }

        let severity = message.words[1];
        if severity < minimum_severity {
            continue;
        }

        sequence = sequence.saturating_add(1);
        let _ = rt::console_write_record(
            console_handle,
            service_id_from_word(message.words[0]),
            severity_from_word(message.words[1]),
            domain_from_word(message.words[2]),
            event_from_word(message.words[3]),
            message.words[4],
            message.words[5],
            sequence,
        );
    }
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
        _ => LogEvent::LookupGranted,
    }
}
