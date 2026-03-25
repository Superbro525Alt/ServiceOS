#![no_std]
#![no_main]

use serviceos_userspace_runtime as rt;
use rt::{ConsoleTag, LogDomain, LogEvent, LogSeverity, RawMessage, ServiceId};

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf301;
    }

    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return 0xf302,
    };
    if rt::register_service(bootstrap, ServiceId::Console, public.second).is_err() {
        return 0xf303;
    }
    let _ = rt::handle_close(public.second);

    loop {
        let mut message = RawMessage::empty(0);
        if rt::channel_receive_blocking(public.first, &mut message).is_err() {
            return 0xf304;
        }
        if message.tag != ConsoleTag::WriteRecord as u32 || message.word_count < 7 {
            continue;
        }

        let source = service_id_from_word(message.words[0]);
        let severity = severity_from_word(message.words[1]);
        let domain = domain_from_word(message.words[2]);
        let event = event_from_word(message.words[3]);
        let _ = match event {
            LogEvent::ServiceStarted | LogEvent::ServiceReady | LogEvent::ServiceRestarting => {
                rt::write_logf(
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} service={} detail={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        service_name(service_id_from_word(message.words[4])),
                        message.words[5],
                    ),
                )
            }
            LogEvent::ServiceFailed => rt::write_logf(
                "console",
                format_args!(
                    "seq={} level={} source={} domain={} event={} service={} exit={}",
                    message.words[6],
                    severity_name(severity),
                    service_name(source),
                    domain_name(domain),
                    event_name(event),
                    service_name(service_id_from_word(message.words[4])),
                    message.words[5],
                ),
            ),
            LogEvent::LookupGranted => rt::write_logf(
                "console",
                format_args!(
                    "seq={} level={} source={} domain={} event={} requester={} target={}",
                    message.words[6],
                    severity_name(severity),
                    service_name(source),
                    domain_name(domain),
                    event_name(event),
                    service_name(service_id_from_word(message.words[4])),
                    service_name(service_id_from_word(message.words[5])),
                ),
            ),
            LogEvent::ConfigLoaded => rt::write_logf(
                "console",
                format_args!(
                    "seq={} level={} source={} domain={} event={} minimum-severity={}",
                    message.words[6],
                    severity_name(severity),
                    service_name(source),
                    domain_name(domain),
                    event_name(event),
                    message.words[4],
                ),
            ),
            LogEvent::StatusStarted => rt::write_logf(
                "console",
                format_args!(
                    "seq={} level={} source={} domain={} event={} heartbeat-ticks={} console-period={}",
                    message.words[6],
                    severity_name(severity),
                    service_name(source),
                    domain_name(domain),
                    event_name(event),
                    message.words[4],
                    message.words[5],
                ),
            ),
            LogEvent::StatusHeartbeat | LogEvent::ConsoleWrite => rt::write_logf(
                "console",
                format_args!(
                    "seq={} level={} source={} domain={} event={} count={} tick={}",
                    message.words[6],
                    severity_name(severity),
                    service_name(source),
                    domain_name(domain),
                    event_name(event),
                    message.words[4],
                    message.words[5],
                ),
            ),
            LogEvent::ConfigRead => rt::write_logf(
                "console",
                format_args!(
                    "seq={} level={} source={} domain={} event={} key={} value={}",
                    message.words[6],
                    severity_name(severity),
                    service_name(source),
                    domain_name(domain),
                    event_name(event),
                    message.words[4],
                    message.words[5],
                ),
            ),
            LogEvent::StorageMounted | LogEvent::ManifestLoaded | LogEvent::ResourceOpened => rt::write_logf(
                "console",
                format_args!(
                    "seq={} level={} source={} domain={} event={} detail0={} detail1={}",
                    message.words[6],
                    severity_name(severity),
                    service_name(source),
                    domain_name(domain),
                    event_name(event),
                    message.words[4],
                    message.words[5],
                ),
            ),
        };
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

fn domain_name(domain: LogDomain) -> &'static str {
    match domain {
        LogDomain::Bootstrap => "bootstrap",
        LogDomain::ServiceManager => "service-manager",
        LogDomain::Service => "service",
        LogDomain::Storage => "storage",
        LogDomain::Log => "log",
        LogDomain::Config => "config",
        LogDomain::Console => "console",
        LogDomain::Status => "status",
        LogDomain::Ipc => "ipc",
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
