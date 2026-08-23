#![no_std]
#![no_main]

use core::fmt::Write;

use rt::{
    ConfigKey, ConfigStatus, ConfigTag, ConfigValueKind, ControlTag, LifecycleEvent, RawMessage,
    ServiceId, StorageEntryKind,
};
use serviceos_userspace_runtime as rt;

const MAX_CONFIG_BYTES: usize = 512;
const MAX_CONFIG_ENTRIES: usize = 14;
const MAX_CONFIG_PATH: usize = 64;

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
    }; 14];
    let mut entry_count = match parse_config_entries(&config_bytes[..loaded], &mut entries) {
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

    let storage_handle = rt::lookup_service(bootstrap, ServiceId::Storage).ok();
    if let Some(storage) = storage_handle {
        if load_override_entries(storage, &mut entries, &mut entry_count).is_err() {
            let _ = rt::write_logf("config", format_args!("override load failed"));
        }
    }

    loop {
        let mut did_work = false;
        match poll_lifecycle(bootstrap) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(_) => return 0xf207,
        }

        let mut request = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut request) {
            Ok(()) if request.tag == ConfigTag::ReadRequest as u32 => {
                did_work = true;
                if request.word_count < 1 || request.handle_count < 1 {
                    continue;
                }

                let reply_handle = request.handles[0];
                let (kind, value) = match find_config(
                    &entries[..entry_count],
                    config_key_from_word(request.words[0]),
                ) {
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
            Ok(()) if request.tag == ConfigTag::WriteRequest as u32 => {
                did_work = true;
                if request.word_count < 2 || request.handle_count < 1 {
                    continue;
                }
                let reply_handle = request.handles[0];
                let key = config_key_from_word(request.words[0]);
                let value = request.words[1];
                let status = if !validate_config_value(key, value) {
                    ConfigStatus::Invalid
                } else if let Some(index) = find_config_index(&entries[..entry_count], key) {
                    entries[index].value = value;
                    match storage_handle {
                        Some(storage)
                            if persist_namespace(storage, &entries[..entry_count], key).is_ok() =>
                        {
                            ConfigStatus::Ok
                        }
                        Some(_) => ConfigStatus::Denied,
                        None => ConfigStatus::Denied,
                    }
                } else if entry_count < entries.len() {
                    entries[entry_count] = ConfigEntry {
                        key,
                        kind: ConfigValueKind::Unsigned,
                        value,
                    };
                    entry_count += 1;
                    match storage_handle {
                        Some(storage)
                            if persist_namespace(storage, &entries[..entry_count], key).is_ok() =>
                        {
                            ConfigStatus::Ok
                        }
                        Some(_) => ConfigStatus::Denied,
                        None => ConfigStatus::Denied,
                    }
                } else {
                    ConfigStatus::Denied
                };

                let mut reply = RawMessage::empty(ConfigTag::WriteReply as u32);
                reply.word_count = 1;
                reply.words[0] = status as u32 as u64;
                let _ = rt::channel_send(reply_handle, &reply);
                let _ = rt::handle_close(reply_handle);
            }
            Ok(()) => {}
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => return 0xf208,
        }

        if !did_work && rt::yield_current().is_err() {
            return 0xf209;
        }
    }
}

fn parse_config_entries(bytes: &[u8], entries: &mut [ConfigEntry]) -> rt::Result<usize> {
    let text = core::str::from_utf8(bytes).map_err(|_| rt::Error::InvalidArgument)?;
    let mut count = 0usize;
    for line in text
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
    {
        let Some((key, value)) = line.split_once('=') else {
            return Err(rt::Error::InvalidArgument);
        };
        if key.trim() == "version" {
            continue;
        }
        if count == entries.len() {
            return Err(rt::Error::CapacityExceeded);
        }
        entries[count] = ConfigEntry {
            key: match key.trim() {
                "log.minimum_severity" => ConfigKey::LogMinimumSeverity,
                "status.heartbeat_ticks" => ConfigKey::StatusHeartbeatTicks,
                "status.console_mirror" => ConfigKey::StatusConsoleMirror,
                "status.heartbeat_log_period" => ConfigKey::StatusHeartbeatLogPeriod,
                "network.ipv4_address" => ConfigKey::NetworkIpv4Address,
                "network.ipv4_prefix_length" => ConfigKey::NetworkIpv4PrefixLength,
                "network.ipv4_gateway" => ConfigKey::NetworkIpv4Gateway,
                "network.probe_timeout_ticks" => ConfigKey::NetworkProbeTimeoutTicks,
                "network.dynamic_ipv4" => ConfigKey::NetworkDynamicIpv4,
                "network.dns_server" => ConfigKey::NetworkDnsServer,
                "network.dns_query_timeout_ticks" => ConfigKey::NetworkDnsQueryTimeoutTicks,
                "network.dhcp_acquire_timeout_ticks" => ConfigKey::NetworkDhcpAcquireTimeoutTicks,
                "network.tcp_connect_timeout_ticks" => ConfigKey::NetworkTcpConnectTimeoutTicks,
                "network.tcp_idle_timeout_ticks" => ConfigKey::NetworkTcpIdleTimeoutTicks,
                _ => return Err(rt::Error::InvalidArgument),
            },
            kind: ConfigValueKind::Unsigned,
            value: value
                .trim()
                .parse::<u64>()
                .map_err(|_| rt::Error::InvalidArgument)?,
        };
        count += 1;
    }
    Ok(count)
}

fn find_config(entries: &[ConfigEntry], key: ConfigKey) -> Option<ConfigEntry> {
    entries.iter().copied().find(|entry| entry.key == key)
}

fn find_config_index(entries: &[ConfigEntry], key: ConfigKey) -> Option<usize> {
    entries.iter().position(|entry| entry.key == key)
}

fn config_key_from_word(value: u64) -> ConfigKey {
    match value as u32 {
        x if x == ConfigKey::LogMinimumSeverity as u32 => ConfigKey::LogMinimumSeverity,
        x if x == ConfigKey::NetworkIpv4Address as u32 => ConfigKey::NetworkIpv4Address,
        x if x == ConfigKey::NetworkIpv4PrefixLength as u32 => ConfigKey::NetworkIpv4PrefixLength,
        x if x == ConfigKey::NetworkIpv4Gateway as u32 => ConfigKey::NetworkIpv4Gateway,
        x if x == ConfigKey::NetworkProbeTimeoutTicks as u32 => ConfigKey::NetworkProbeTimeoutTicks,
        x if x == ConfigKey::NetworkDynamicIpv4 as u32 => ConfigKey::NetworkDynamicIpv4,
        x if x == ConfigKey::NetworkDnsServer as u32 => ConfigKey::NetworkDnsServer,
        x if x == ConfigKey::NetworkDnsQueryTimeoutTicks as u32 => {
            ConfigKey::NetworkDnsQueryTimeoutTicks
        }
        x if x == ConfigKey::NetworkDhcpAcquireTimeoutTicks as u32 => {
            ConfigKey::NetworkDhcpAcquireTimeoutTicks
        }
        x if x == ConfigKey::NetworkTcpConnectTimeoutTicks as u32 => {
            ConfigKey::NetworkTcpConnectTimeoutTicks
        }
        x if x == ConfigKey::NetworkTcpIdleTimeoutTicks as u32 => {
            ConfigKey::NetworkTcpIdleTimeoutTicks
        }
        x if x == ConfigKey::StatusConsoleMirror as u32 => ConfigKey::StatusConsoleMirror,
        x if x == ConfigKey::StatusHeartbeatLogPeriod as u32 => ConfigKey::StatusHeartbeatLogPeriod,
        _ => ConfigKey::StatusHeartbeatTicks,
    }
}

fn config_key_name(key: ConfigKey) -> &'static str {
    match key {
        ConfigKey::LogMinimumSeverity => "log.minimum_severity",
        ConfigKey::StatusHeartbeatTicks => "status.heartbeat_ticks",
        ConfigKey::StatusConsoleMirror => "status.console_mirror",
        ConfigKey::StatusHeartbeatLogPeriod => "status.heartbeat_log_period",
        ConfigKey::NetworkIpv4Address => "network.ipv4_address",
        ConfigKey::NetworkIpv4PrefixLength => "network.ipv4_prefix_length",
        ConfigKey::NetworkIpv4Gateway => "network.ipv4_gateway",
        ConfigKey::NetworkProbeTimeoutTicks => "network.probe_timeout_ticks",
        ConfigKey::NetworkDynamicIpv4 => "network.dynamic_ipv4",
        ConfigKey::NetworkDnsServer => "network.dns_server",
        ConfigKey::NetworkDnsQueryTimeoutTicks => "network.dns_query_timeout_ticks",
        ConfigKey::NetworkDhcpAcquireTimeoutTicks => "network.dhcp_acquire_timeout_ticks",
        ConfigKey::NetworkTcpConnectTimeoutTicks => "network.tcp_connect_timeout_ticks",
        ConfigKey::NetworkTcpIdleTimeoutTicks => "network.tcp_idle_timeout_ticks",
    }
}

fn namespace_for_key(key: ConfigKey) -> &'static str {
    match key {
        ConfigKey::LogMinimumSeverity => "log",
        ConfigKey::StatusHeartbeatTicks
        | ConfigKey::StatusConsoleMirror
        | ConfigKey::StatusHeartbeatLogPeriod => "status",
        _ => "network",
    }
}

fn validate_config_value(key: ConfigKey, value: u64) -> bool {
    match key {
        ConfigKey::LogMinimumSeverity => value <= 3,
        ConfigKey::StatusConsoleMirror | ConfigKey::NetworkDynamicIpv4 => value <= 1,
        ConfigKey::NetworkIpv4PrefixLength => value <= 32,
        _ => true,
    }
}

fn load_override_entries(
    storage_handle: rt::Handle,
    entries: &mut [ConfigEntry; MAX_CONFIG_ENTRIES],
    entry_count: &mut usize,
) -> rt::Result<()> {
    for namespace in ["log", "status", "network"] {
        let mut path = rt::FixedLogBuffer::<MAX_CONFIG_PATH>::new();
        let _ = core::fmt::write(
            &mut path,
            format_args!("state/config/{}/settings.cfg", namespace),
        );
        let Ok((blob, len)) = rt::storage_open(storage_handle, path.as_str()) else {
            continue;
        };
        let mut bytes = [0u8; MAX_CONFIG_BYTES];
        let requested = len.min(bytes.len());
        let loaded = rt::storage_read_all(blob, &mut bytes, requested)?;
        let _ = rt::storage_blob_close(blob);
        let mut overrides = [ConfigEntry {
            key: ConfigKey::LogMinimumSeverity,
            kind: ConfigValueKind::Unsigned,
            value: 0,
        }; MAX_CONFIG_ENTRIES];
        let count = parse_config_entries(&bytes[..loaded], &mut overrides)?;
        for override_entry in overrides[..count].iter().copied() {
            if let Some(index) = find_config_index(&entries[..*entry_count], override_entry.key) {
                entries[index] = override_entry;
            } else if *entry_count < entries.len() {
                entries[*entry_count] = override_entry;
                *entry_count += 1;
            }
        }
    }
    Ok(())
}

fn persist_namespace(
    storage_handle: rt::Handle,
    entries: &[ConfigEntry],
    key: ConfigKey,
) -> rt::Result<()> {
    ensure_directory(storage_handle, "state/")?;
    ensure_directory(storage_handle, "state/config/")?;
    let mut namespace_dir = rt::FixedLogBuffer::<MAX_CONFIG_PATH>::new();
    let _ = core::fmt::write(
        &mut namespace_dir,
        format_args!("state/config/{}/", namespace_for_key(key)),
    );
    ensure_directory(storage_handle, namespace_dir.as_str())?;

    let mut path = rt::FixedLogBuffer::<MAX_CONFIG_PATH>::new();
    let _ = core::fmt::write(
        &mut path,
        format_args!("{}settings.cfg", namespace_dir.as_str()),
    );
    let text = serialize_namespace(entries, namespace_for_key(key))?;
    write_storage_file(storage_handle, path.as_str(), text.as_bytes())
}

fn ensure_directory(storage_handle: rt::Handle, path: &str) -> rt::Result<()> {
    if path.is_empty() {
        return Ok(());
    }
    if rt::storage_open_directory(storage_handle, path, true).is_ok() {
        return Ok(());
    }
    let mut parent = rt::FixedLogBuffer::<MAX_CONFIG_PATH>::new();
    let name = split_parent_path(path, &mut parent)?;
    let directory = rt::storage_open_directory(storage_handle, parent.as_str(), true)?;
    let result = rt::storage_directory_create(directory, name, StorageEntryKind::Directory);
    let _ = rt::handle_close(directory);
    match result {
        Ok(()) | Err(rt::Error::Busy) => Ok(()),
        Err(error) => Err(error),
    }
}

fn write_storage_file(storage_handle: rt::Handle, path: &str, bytes: &[u8]) -> rt::Result<()> {
    let mut parent = rt::FixedLogBuffer::<MAX_CONFIG_PATH>::new();
    let name = split_parent_path(path, &mut parent)?;
    let directory = rt::storage_open_directory(storage_handle, parent.as_str(), true)?;
    let (file, _) = rt::storage_directory_open_file(directory, name, true, true)?;
    let _ = rt::handle_close(directory);
    let mut offset = 0usize;
    while offset < bytes.len() {
        let chunk_len = (bytes.len() - offset).min((rt::IPC_MAX_WORDS - 3) * 8);
        let _ = rt::storage_write(
            file,
            offset,
            bytes.len(),
            &bytes[offset..offset + chunk_len],
        )?;
        offset += chunk_len;
    }
    let _ = rt::storage_blob_close(file);
    Ok(())
}

fn serialize_namespace(
    entries: &[ConfigEntry],
    namespace: &str,
) -> rt::Result<rt::FixedLogBuffer<MAX_CONFIG_BYTES>> {
    let mut out = rt::FixedLogBuffer::<MAX_CONFIG_BYTES>::new();
    let _ = core::fmt::write(&mut out, format_args!("version=1\n"));
    for entry in entries
        .iter()
        .copied()
        .filter(|entry| namespace_for_key(entry.key) == namespace)
    {
        let _ = core::fmt::write(
            &mut out,
            format_args!("{}={}\n", config_key_name(entry.key), entry.value),
        );
    }
    Ok(out)
}

fn split_parent_path<'a>(
    path: &'a str,
    parent_buffer: &mut rt::FixedLogBuffer<MAX_CONFIG_PATH>,
) -> rt::Result<&'a str> {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        return Err(rt::Error::InvalidArgument);
    }
    match trimmed.rsplit_once('/') {
        Some((parent, name)) if !name.is_empty() => {
            let _ = parent_buffer.write_str(parent);
            let _ = parent_buffer.write_str("/");
            Ok(name)
        }
        Some(_) => Err(rt::Error::InvalidArgument),
        None => Ok(trimmed),
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
