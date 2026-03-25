#![no_std]
#![no_main]

use serviceos_userspace_runtime as rt;
use rt::{ConfigKey, ConfigTag, ConfigValueKind, RawMessage, ServiceId};

#[derive(Clone, Copy)]
struct ConfigEntry {
    key: ConfigKey,
    kind: ConfigValueKind,
    value: u64,
}

const CONFIG_ENTRIES: &[ConfigEntry] = &[
    ConfigEntry {
        key: ConfigKey::LogMinimumSeverity,
        kind: ConfigValueKind::Unsigned,
        value: 3,
    },
    ConfigEntry {
        key: ConfigKey::StatusHeartbeatTicks,
        kind: ConfigValueKind::Unsigned,
        value: 250,
    },
    ConfigEntry {
        key: ConfigKey::StatusConsoleMirror,
        kind: ConfigValueKind::Unsigned,
        value: 2,
    },
];

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf201;
    }

    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return 0xf202,
    };
    if rt::register_service(bootstrap, ServiceId::Config, public.second).is_err() {
        return 0xf203;
    }
    let _ = rt::handle_close(public.second);

    loop {
        let mut request = RawMessage::empty(0);
        if rt::channel_receive_blocking(public.first, &mut request).is_err() {
            return 0xf204;
        }
        if request.tag != ConfigTag::ReadRequest as u32 || request.word_count < 1 || request.handle_count < 1 {
            continue;
        }

        let reply_handle = request.handles[0];
        let (kind, value) = match find_config(config_key_from_word(request.words[0])) {
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
}

fn find_config(key: ConfigKey) -> Option<ConfigEntry> {
    CONFIG_ENTRIES.iter().copied().find(|entry| entry.key == key)
}

fn config_key_from_word(value: u64) -> ConfigKey {
    match value as u32 {
        x if x == ConfigKey::LogMinimumSeverity as u32 => ConfigKey::LogMinimumSeverity,
        x if x == ConfigKey::StatusConsoleMirror as u32 => ConfigKey::StatusConsoleMirror,
        _ => ConfigKey::StatusHeartbeatTicks,
    }
}
