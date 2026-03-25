#![no_std]
#![no_main]

use serviceos_userspace_runtime as rt;
use rt::{ConfigKey, LogDomain, LogEvent, LogSeverity, RawMessage, ServiceId, StatusTag};

const MAX_BANNER_BYTES: usize = 128;

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
    let _ = rt::handle_close(banner_handle);
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
    let _ = rt::handle_close(config_handle);

    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return 0xf409,
    };
    if rt::register_service(bootstrap, ServiceId::Status, public.second).is_err() {
        return 0xf40a;
    }
    let _ = rt::handle_close(public.second);

    let mut heartbeat_count = 0u64;
    let mut last_tick = 0u64;
    let mut next_heartbeat = match rt::monotonic_now() {
        Ok(now) => now.saturating_add(heartbeat_ticks),
        Err(_) => return 0xf40b,
    };

    let _ = rt::send_log_record(
        log_handle,
        ServiceId::Status,
        LogSeverity::Info,
        LogDomain::Status,
        LogEvent::StatusStarted,
        heartbeat_ticks,
        console_mirror,
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
        let mut request = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut request) {
            Ok(()) => {
                if request.tag == StatusTag::SnapshotRequest as u32 && request.handle_count > 0 {
                    let reply_handle = request.handles[0];
                    let mut reply = RawMessage::empty(StatusTag::SnapshotReply as u32);
                    reply.word_count = 2;
                    reply.words[0] = heartbeat_count;
                    reply.words[1] = last_tick;
                    let _ = rt::channel_send(reply_handle, &reply);
                    let _ = rt::handle_close(reply_handle);
                }
            }
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => return 0xf40c,
        }

        let now = match rt::monotonic_now() {
            Ok(now) => now,
            Err(_) => return 0xf40d,
        };
        if now >= next_heartbeat {
            heartbeat_count = heartbeat_count.saturating_add(1);
            last_tick = now;
            next_heartbeat = now.saturating_add(heartbeat_ticks);
            let _ = rt::send_log_record(
                log_handle,
                ServiceId::Status,
                LogSeverity::Info,
                LogDomain::Status,
                LogEvent::StatusHeartbeat,
                heartbeat_count,
                last_tick,
            );
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

        if rt::yield_current().is_err() {
            return 0xf40e;
        }
    }
}
