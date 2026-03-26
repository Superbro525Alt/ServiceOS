#![no_std]
#![no_main]

use serviceos_userspace_runtime as rt;
use rt::{ConfigKey, ConfigTag, ConfigValueKind, ControlTag, LifecycleEvent, RawMessage, ServiceId};

const MAX_CONFIG_BYTES: usize = 256;

#[derive(Clone, Copy)]
struct ConfigEntry {
    key: ConfigKey,
    kind: ConfigValueKind,
    value: u64,
}

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf201;
    }
    if startup.handle_count < 1 || startup.word_count < 5 {
        return 0xf202;
    }

    let config_blob = startup.handles[startup.words[2] as usize];
    let config_len = startup.words[4] as usize;
    let mut config_bytes = [0u8; MAX_CONFIG_BYTES];
    let requested = config_len.min(config_bytes.len());
    let loaded = match rt::storage_read_all(config_blob, &mut config_bytes, requested) {
        Ok(loaded) => loaded,
        Err(_) => return 0xf203,
    };
    let _ = rt::storage_blob_close(config_blob);

    let mut entries = [ConfigEntry {
        key: ConfigKey::LogMinimumSeverity,
        kind: ConfigValueKind::Unsigned,
        value: 0,
    }; 4];
    let entry_count = match parse_config_entries(&config_bytes[..loaded], &mut entries) {
        Ok(count) => count,
        Err(_) => return 0xf204,
    };

    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return 0xf205,
    };
    if rt::register_service(bootstrap, ServiceId::Config, public.second).is_err() {
        return 0xf206;
    }
    let _ = rt::handle_close(public.second);

    loop {
        match poll_lifecycle(bootstrap) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(_) => return 0xf207,
        }

        let mut request = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut request) {
            Ok(()) => {
                if request.tag != ConfigTag::ReadRequest as u32
                    || request.word_count < 1
                    || request.handle_count < 1
                {
                    continue;
                }

                let reply_handle = request.handles[0];
                let (kind, value) =
                    match find_config(&entries[..entry_count], config_key_from_word(request.words[0])) {
                        Some(entry) => (entry.kind as u32 as u64, entry.value),
                        None => (0, 0),
                    };

                let mut reply = RawMessage::empty(ConfigTag::ReadReply as u32);
                reply.word_count = 3;
                reply.words[0] = request.words[0];
                reply.words[1] = kind;
                reply.words[2] = value;
                let _ = rt::channel_send(reply_handle, &reply);
                let _ = rt::handle_close(reply_handle);
            }
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => return 0xf208,
        }

        if rt::yield_current().is_err() {
            return 0xf209;
        }
    }
}

fn parse_config_entries(bytes: &[u8], entries: &mut [ConfigEntry]) -> rt::Result<usize> {
    let text = core::str::from_utf8(bytes).map_err(|_| rt::Error::InvalidArgument)?;
    let mut count = 0usize;
    for line in text.lines().map(|line| line.trim()).filter(|line| !line.is_empty()) {
        let Some((key, value)) = line.split_once('=') else {
            return Err(rt::Error::InvalidArgument);
        };
        if count == entries.len() {
            return Err(rt::Error::CapacityExceeded);
        }
        entries[count] = ConfigEntry {
            key: match key.trim() {
                "log.minimum_severity" => ConfigKey::LogMinimumSeverity,
                "status.heartbeat_ticks" => ConfigKey::StatusHeartbeatTicks,
                "status.console_mirror" => ConfigKey::StatusConsoleMirror,
                "status.heartbeat_log_period" => ConfigKey::StatusHeartbeatLogPeriod,
                _ => return Err(rt::Error::InvalidArgument),
            },
            kind: ConfigValueKind::Unsigned,
            value: value.trim().parse::<u64>().map_err(|_| rt::Error::InvalidArgument)?,
        };
        count += 1;
    }
    Ok(count)
}

fn find_config(entries: &[ConfigEntry], key: ConfigKey) -> Option<ConfigEntry> {
    entries.iter().copied().find(|entry| entry.key == key)
}

fn config_key_from_word(value: u64) -> ConfigKey {
    match value as u32 {
        x if x == ConfigKey::LogMinimumSeverity as u32 => ConfigKey::LogMinimumSeverity,
        x if x == ConfigKey::StatusConsoleMirror as u32 => ConfigKey::StatusConsoleMirror,
        x if x == ConfigKey::StatusHeartbeatLogPeriod as u32 => ConfigKey::StatusHeartbeatLogPeriod,
        _ => ConfigKey::StatusHeartbeatTicks,
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
