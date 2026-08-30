use rt::{RawMessage, TerminalStatus, TerminalTag};
use serviceos_userspace_runtime as rt;

pub(crate) const PROFILE_NAME_BYTES: usize = 10;
pub(crate) const PROFILE_PROGRAM_BYTES: usize = 18;
pub(crate) const PROFILE_ARGS_BYTES: usize = 22;
pub(crate) const PROFILE_ENV_BYTES: usize = 36;
pub(crate) const PROFILE_CWD_BYTES: usize = 22;
pub(crate) const PROFILE_WIRE_LEN: usize = PROFILE_NAME_BYTES
    + PROFILE_PROGRAM_BYTES
    + PROFILE_ARGS_BYTES
    + PROFILE_ENV_BYTES
    + PROFILE_CWD_BYTES
    + 1;
pub(crate) const PROFILE_COUNT: usize = 3;

/// Durable profile store location on the persistent storage mount, following
/// the `state/<app>/` convention used by desktop-shell's access store.
const STORE_DIR_PATH: &str = "state/terminal/";
const STORE_ROOT_PATH: &str = "state/";
const STORE_DIR_NAME: &str = "terminal";
const STORE_FILE_NAME: &str = "profiles.cfg";
const STORE_FILE_MAX_BYTES: usize = 512;

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub(crate) struct TerminalProfile {
    pub(crate) name: [u8; PROFILE_NAME_BYTES],
    pub(crate) name_len: usize,
    pub(crate) program: [u8; PROFILE_PROGRAM_BYTES],
    pub(crate) program_len: usize,
    pub(crate) args: [u8; PROFILE_ARGS_BYTES],
    pub(crate) args_len: usize,
    pub(crate) env: [u8; PROFILE_ENV_BYTES],
    pub(crate) env_len: usize,
    pub(crate) cwd: [u8; PROFILE_CWD_BYTES],
    pub(crate) cwd_len: usize,
    pub(crate) theme_index: u8,
}

#[allow(dead_code)]
fn copy_field<const N: usize>(destination: &mut [u8; N], source: &[u8]) -> usize {
    let len = source.len().min(N);
    destination[..len].copy_from_slice(&source[..len]);
    if len < N {
        destination[len..].fill(0);
    }
    len
}

/// Named launch profiles applied to new tabs and splits. Theme colors come from
/// the profile; shell program/args/env/cwd are relayed to terminal-service.
pub(crate) static DEFAULT_PROFILES: [TerminalProfile; PROFILE_COUNT] = [
    TerminalProfile::build("STANDARD", "builtin-sh", "", "TERM=serviceos", "/", 0),
    TerminalProfile::build("DEV", "builtin-sh", "", "TERM=serviceos", "/config", 1),
    TerminalProfile::build("AMBER", "builtin-sh", "", "TERM=serviceos", "/", 2),
];

pub(crate) fn encode_wire(profile: &TerminalProfile, out: &mut [u8; PROFILE_WIRE_LEN]) {
    out[..PROFILE_NAME_BYTES].copy_from_slice(&profile.name);
    let mut offset = PROFILE_NAME_BYTES;
    out[offset..offset + PROFILE_PROGRAM_BYTES].copy_from_slice(&profile.program);
    offset += PROFILE_PROGRAM_BYTES;
    out[offset..offset + PROFILE_ARGS_BYTES].copy_from_slice(&profile.args);
    offset += PROFILE_ARGS_BYTES;
    out[offset..offset + PROFILE_ENV_BYTES].copy_from_slice(&profile.env);
    offset += PROFILE_ENV_BYTES;
    out[offset..offset + PROFILE_CWD_BYTES].copy_from_slice(&profile.cwd);
    offset += PROFILE_CWD_BYTES;
    out[offset] = profile.theme_index;
}

#[allow(dead_code)]
pub(crate) fn decode_wire(bytes: &[u8]) -> Option<TerminalProfile> {
    if bytes.len() < PROFILE_WIRE_LEN {
        return None;
    }
    let mut profile = TerminalProfile::empty();
    profile.name.copy_from_slice(&bytes[..PROFILE_NAME_BYTES]);
    let mut offset = PROFILE_NAME_BYTES;
    profile
        .program
        .copy_from_slice(&bytes[offset..offset + PROFILE_PROGRAM_BYTES]);
    offset += PROFILE_PROGRAM_BYTES;
    profile
        .args
        .copy_from_slice(&bytes[offset..offset + PROFILE_ARGS_BYTES]);
    offset += PROFILE_ARGS_BYTES;
    profile
        .env
        .copy_from_slice(&bytes[offset..offset + PROFILE_ENV_BYTES]);
    offset += PROFILE_ENV_BYTES;
    profile
        .cwd
        .copy_from_slice(&bytes[offset..offset + PROFILE_CWD_BYTES]);
    offset += PROFILE_CWD_BYTES;
    profile.theme_index = bytes[offset];
    profile.name_len = cstr_len(&profile.name);
    profile.program_len = cstr_len(&profile.program);
    profile.args_len = cstr_len(&profile.args);
    profile.env_len = cstr_len(&profile.env);
    profile.cwd_len = cstr_len(&profile.cwd);
    Some(profile)
}

impl TerminalProfile {
    pub(crate) const fn empty() -> Self {
        Self {
            name: [0; PROFILE_NAME_BYTES],
            name_len: 0,
            program: [0; PROFILE_PROGRAM_BYTES],
            program_len: 0,
            args: [0; PROFILE_ARGS_BYTES],
            args_len: 0,
            env: [0; PROFILE_ENV_BYTES],
            env_len: 0,
            cwd: [0; PROFILE_CWD_BYTES],
            cwd_len: 0,
            theme_index: 0,
        }
    }

    /// Const builder so profiles can live in statics.
    const fn build(
        name: &str,
        program: &str,
        args: &str,
        env: &str,
        cwd: &str,
        theme_index: u8,
    ) -> Self {
        let mut profile = Self::empty();
        profile.name_len = const_copy(&mut profile.name, name.as_bytes());
        profile.program_len = const_copy(&mut profile.program, program.as_bytes());
        profile.args_len = const_copy(&mut profile.args, args.as_bytes());
        profile.env_len = const_copy(&mut profile.env, env.as_bytes());
        profile.cwd_len = const_copy(&mut profile.cwd, cwd.as_bytes());
        profile.theme_index = theme_index;
        profile
    }

    pub(crate) fn name_str(&self) -> &str {
        let end = self.name_len.min(self.name.len());
        core::str::from_utf8(&self.name[..end]).unwrap_or("")
    }
}

const fn const_copy<const N: usize>(destination: &mut [u8; N], source: &[u8]) -> usize {
    let mut index = 0usize;
    while index < N && index < source.len() {
        destination[index] = source[index];
        index += 1;
    }
    index
}

#[allow(dead_code)]
fn cstr_len(field: &[u8]) -> usize {
    field
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(field.len())
}

/// Load the persisted profile set from storage. Returns None on any missing
/// piece (no store file yet, transport error, malformed text) so the caller
/// falls back to the built-in defaults.
pub(crate) fn load_profiles(storage: rt::Handle) -> Option<[TerminalProfile; PROFILE_COUNT]> {
    let (blob, size) = rt::storage_open(storage, "state/terminal/profiles.cfg").ok()?;
    let mut buffer = [0u8; STORE_FILE_MAX_BYTES];
    let want = size.min(buffer.len());
    let read = rt::storage_read_all(blob, &mut buffer, want);
    let _ = rt::storage_blob_close(blob);
    let read = read.ok()?;
    parse_config_text(&buffer[..read])
}

/// Persist the profile set (including each profile's theme index) durably.
/// Creates the `state/terminal/` directory when missing. Best effort: returns
/// whether the write landed.
pub(crate) fn store_profiles(
    storage: rt::Handle,
    profiles: &[TerminalProfile; PROFILE_COUNT],
) -> bool {
    if rt::storage_open_directory(storage, STORE_DIR_PATH, true).is_err() {
        let Ok(root) = rt::storage_open_directory(storage, STORE_ROOT_PATH, true) else {
            return false;
        };
        let created =
            rt::storage_directory_create(root, STORE_DIR_NAME, rt::StorageEntryKind::Directory);
        let _ = rt::handle_close(root);
        if created.is_err() && rt::storage_open_directory(storage, STORE_DIR_PATH, true).is_err() {
            return false;
        }
    }
    let Ok(dir) = rt::storage_open_directory(storage, STORE_DIR_PATH, true) else {
        return false;
    };
    let file = rt::storage_directory_open_file(dir, STORE_FILE_NAME, true, true);
    let _ = rt::handle_close(dir);
    let Ok((blob, _)) = file else {
        return false;
    };
    let mut buffer = [0u8; STORE_FILE_MAX_BYTES];
    let len = write_config_text(profiles, &mut buffer);
    let written = rt::storage_write(blob, 0, len, &buffer[..len]);
    let _ = rt::storage_blob_close(blob);
    written.is_ok()
}

/// Serialize profiles in the config-service `key=value` line style.
#[allow(dead_code)]
pub(crate) fn write_config_text(profiles: &[TerminalProfile], out: &mut [u8]) -> usize {
    let mut written = 0usize;
    fn push(out: &mut [u8], written: &mut usize, bytes: &[u8]) {
        for byte in bytes {
            if *written < out.len() {
                out[*written] = *byte;
                *written += 1;
            }
        }
    }
    let mut number = [0u8; 4];
    for (index, profile) in profiles.iter().enumerate() {
        let index_text = format_index(index, &mut number);
        let entries: [(&str, Option<&[u8]>); 6] = [
            ("name", Some(&profile.name[..profile.name_len])),
            ("program", Some(&profile.program[..profile.program_len])),
            ("args", Some(&profile.args[..profile.args_len])),
            ("env", Some(&profile.env[..profile.env_len])),
            ("cwd", Some(&profile.cwd[..profile.cwd_len])),
            ("theme", None),
        ];
        for (key, value) in entries {
            push(out, &mut written, b"p");
            push(out, &mut written, index_text);
            push(out, &mut written, b".");
            push(out, &mut written, key.as_bytes());
            push(out, &mut written, b"=");
            match value {
                Some(bytes) => push(out, &mut written, bytes),
                None => {
                    let mut digit = [0u8; 3];
                    let theme_text = format_index(profile.theme_index as usize, &mut digit);
                    push(out, &mut written, theme_text);
                }
            }
            push(out, &mut written, b"\n");
        }
    }
    written
}

#[allow(dead_code)]
fn format_index(value: usize, buffer: &mut [u8]) -> &[u8] {
    let mut len = 0usize;
    let mut rest = value;
    if rest == 0 {
        buffer[0] = b'0';
        return &buffer[..1];
    }
    while rest > 0 && len < buffer.len() {
        buffer[len] = b'0' + (rest % 10) as u8;
        len += 1;
        rest /= 10;
    }
    buffer[..len].reverse();
    &buffer[..len]
}

/// Parse the config-service style `key=value` text back into profiles.
/// Returns None when any line is malformed.
#[allow(dead_code)]
pub(crate) fn parse_config_text(bytes: &[u8]) -> Option<[TerminalProfile; PROFILE_COUNT]> {
    let mut profiles = [TerminalProfile::empty(); PROFILE_COUNT];
    let mut seen = [false; PROFILE_COUNT];
    for line in bytes.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        let text = core::str::from_utf8(line).ok()?;
        let (key, value) = text.split_once('=')?;
        let mut parts = key.split('.');
        let index_text = parts.next()?;
        let field = parts.next()?;
        if parts.next().is_some() || !index_text.starts_with('p') {
            return None;
        }
        let index: usize = index_text[1..].parse().ok()?;
        if index >= PROFILE_COUNT {
            return None;
        }
        let profile = &mut profiles[index];
        match field {
            "name" => profile.name_len = copy_field(&mut profile.name, value.as_bytes()),
            "program" => profile.program_len = copy_field(&mut profile.program, value.as_bytes()),
            "args" => profile.args_len = copy_field(&mut profile.args, value.as_bytes()),
            "env" => profile.env_len = copy_field(&mut profile.env, value.as_bytes()),
            "cwd" => profile.cwd_len = copy_field(&mut profile.cwd, value.as_bytes()),
            "theme" => {
                let parsed: usize = value.parse().ok()?;
                if parsed > u8::MAX as usize {
                    return None;
                }
                profile.theme_index = parsed as u8;
            }
            _ => return None,
        }
        seen[index] = true;
    }
    seen.iter().all(|seen| *seen).then_some(profiles)
}

/// Open a terminal session carrying this profile's launch metadata to
/// terminal-service. Mirrors rt::terminal_session_open plus profile payload
/// words (len + packed bytes).
pub(crate) fn open_session_with_profile(
    service_handle: rt::Handle,
    profile: &TerminalProfile,
) -> rt::Result<(u32, rt::Handle, u32, u32)> {
    let reply = rt::channel_create()?;
    let mut request = RawMessage::empty(TerminalTag::SessionOpenRequest as u32);
    request.word_count = 1 + ((PROFILE_WIRE_LEN + 7) / 8) as u32;
    request.words[0] = PROFILE_WIRE_LEN as u64;
    let mut wire = [0u8; PROFILE_WIRE_LEN];
    encode_wire(profile, &mut wire);
    for (index, chunk) in wire.chunks(8).enumerate() {
        let mut bytes = [0u8; 8];
        bytes[..chunk.len()].copy_from_slice(chunk);
        request.words[1 + index] = u64::from_le_bytes(bytes);
    }
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rt::rights::SEND;
    rt::channel_send(service_handle, &request)?;
    let _ = rt::handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    rt::channel_receive_blocking(reply.first, &mut response)?;
    let _ = rt::handle_close(reply.first);
    if response.tag != TerminalTag::SessionOpenReply as u32 || response.word_count < 4 {
        return Err(rt::Error::InvalidArgument);
    }
    match response.words[0] as u32 {
        x if x == TerminalStatus::Ok as u32 && response.handle_count > 0 => Ok((
            response.words[1] as u32,
            response.handles[0],
            response.words[2] as u32,
            response.words[3] as u32,
        )),
        _ => Err(rt::Error::PermissionDenied),
    }
}

/// Attach to a detached session by id, receiving a fresh client handle.
/// Mirrors open_session_with_profile's reply-channel pattern.
pub(crate) fn attach_to_session(
    service_handle: rt::Handle,
    session_id: u32,
) -> rt::Result<rt::Handle> {
    let reply = rt::channel_create()?;
    let mut request = RawMessage::empty(crate::wire::SESSION_ATTACH_REQUEST);
    request.word_count = 1;
    request.words[0] = session_id as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rt::rights::SEND;
    rt::channel_send(service_handle, &request)?;
    let _ = rt::handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    rt::channel_receive_blocking(reply.first, &mut response)?;
    let _ = rt::handle_close(reply.first);
    if response.tag != crate::wire::SESSION_ATTACH_REPLY || response.word_count < 2 {
        return Err(rt::Error::InvalidArgument);
    }
    match response.words[0] as u32 {
        x if x == TerminalStatus::Ok as u32 && response.handle_count > 0 => Ok(response.handles[0]),
        _ => Err(rt::Error::PermissionDenied),
    }
}

/// Enumerate live sessions as (id, attached) pairs; returns the count written
/// into `out`. Detached entries are the reattachable ones.
pub(crate) fn enumerate_sessions(
    service_handle: rt::Handle,
    out: &mut [(u32, bool)],
) -> rt::Result<usize> {
    let reply = rt::channel_create()?;
    let mut request = RawMessage::empty(crate::wire::SESSION_ENUMERATE_REQUEST);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rt::rights::SEND;
    rt::channel_send(service_handle, &request)?;
    let _ = rt::handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    rt::channel_receive_blocking(reply.first, &mut response)?;
    let _ = rt::handle_close(reply.first);
    if response.tag != crate::wire::SESSION_ENUMERATE_REPLY || response.word_count < 2 {
        return Err(rt::Error::InvalidArgument);
    }
    if (response.words[0] as u32) != TerminalStatus::Ok as u32 {
        return Err(rt::Error::PermissionDenied);
    }
    let count = (response.words[1] as usize).min(out.len());
    for index in 0..count {
        let id = response.words[2 + index * 2] as u32;
        let attached = response.words[3 + index * 2] != 0;
        out[index] = (id, attached);
    }
    Ok(count)
}

/// Query terminal-service's active theme (service-global operator pick).
/// Returns None on any transport or reply failure so callers keep their
/// local default; a fresh service answers 0 (the default theme).
pub(crate) fn query_active_theme(service_handle: rt::Handle) -> Option<u8> {
    let reply = rt::channel_create().ok()?;
    let mut request = RawMessage::empty(crate::wire::THEME_GET_REQUEST);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rt::rights::SEND;
    let sent = rt::channel_send(service_handle, &request);
    let _ = rt::handle_close(reply.second);
    sent.ok()?;

    let mut response = RawMessage::empty(0);
    rt::channel_receive_blocking(reply.first, &mut response).ok()?;
    let _ = rt::handle_close(reply.first);
    if response.tag != crate::wire::THEME_GET_REPLY || response.word_count < 2 {
        return None;
    }
    if (response.words[0] as u32) != TerminalStatus::Ok as u32 {
        return None;
    }
    Some(response.words[1] as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::THEMES;

    fn sample() -> TerminalProfile {
        TerminalProfile::build("DEV", "builtin-sh", "-l", "TERM=serviceos", "/config", 1)
    }

    #[test]
    fn wire_roundtrip_preserves_fields() {
        let original = sample();
        let mut wire = [0u8; PROFILE_WIRE_LEN];
        encode_wire(&original, &mut wire);
        let decoded = decode_wire(&wire).expect("decode");
        assert_eq!(decoded, original);
    }

    #[test]
    fn decode_rejects_short_buffer() {
        assert!(decode_wire(&[0u8; PROFILE_WIRE_LEN - 1]).is_none());
        let mut wire = [0u8; PROFILE_WIRE_LEN];
        encode_wire(&sample(), &mut wire);
        assert!(decode_wire(&wire).is_some());
    }

    #[test]
    fn fields_truncate_to_capacity() {
        let long = TerminalProfile::build(
            "WAY_TOO_LONG_PROFILE_NAME",
            "program-longer-than-eighteen-bytes!",
            "arg arg arg arg arg arg arg arg arg arg",
            "K1=V1;K2=V2;K3=V3;K4=V4;K5=V5;K6=V6;K7",
            "/very/deeply/nested/directory/path/here",
            2,
        );
        assert_eq!(long.name_len, PROFILE_NAME_BYTES);
        assert_eq!(long.program_len, PROFILE_PROGRAM_BYTES);
        assert_eq!(long.args_len, PROFILE_ARGS_BYTES);
        assert_eq!(long.env_len, PROFILE_ENV_BYTES);
        assert_eq!(long.cwd_len, PROFILE_CWD_BYTES);
        let mut wire = [0u8; PROFILE_WIRE_LEN];
        encode_wire(&long, &mut wire);
        assert_eq!(decode_wire(&wire).unwrap(), long);
    }

    #[test]
    fn default_profiles_have_distinct_names_and_themes() {
        for (index, profile) in DEFAULT_PROFILES.iter().enumerate() {
            assert!(!profile.name_str().is_empty(), "profile {index} unnamed");
            assert_eq!(
                profile.theme_index as usize % THEMES.len(),
                index % THEMES.len()
            );
        }
        assert!(DEFAULT_PROFILES[0].name_str() != DEFAULT_PROFILES[1].name_str());
    }

    #[test]
    fn config_text_roundtrip() {
        let profiles = [sample(), DEFAULT_PROFILES[1], DEFAULT_PROFILES[2]];
        let mut buffer = [0u8; 512];
        let len = write_config_text(&profiles, &mut buffer);
        assert!(len > 0 && len <= buffer.len());
        let parsed = parse_config_text(&buffer[..len]).expect("parse");
        assert_eq!(parsed, profiles);
    }

    #[test]
    fn theme_codec_roundtrips_every_theme_index() {
        for theme_index in 0..crate::THEMES.len() {
            let mut profiles = DEFAULT_PROFILES;
            profiles[1].theme_index = theme_index as u8;
            let mut buffer = [0u8; 512];
            let len = write_config_text(&profiles, &mut buffer);
            let parsed = parse_config_text(&buffer[..len]).expect("parse");
            assert_eq!(parsed[1].theme_index as usize, theme_index);
            // Theme selection drives rendering through the same modulo.
            assert_eq!(
                parsed[1].theme_index as usize % crate::THEMES.len(),
                theme_index
            );
        }
    }

    #[test]
    fn theme_codec_rejects_out_of_range_values() {
        let mut buffer = [0u8; 512];
        let len = write_config_text(&DEFAULT_PROFILES, &mut buffer);
        // Corrupt the DEV profile's theme value past u8 range; the trailing
        // byte eats the newline so the value swallows the next key.
        let marker = b"p1.theme=1\n";
        let start = buffer[..len]
            .windows(marker.len())
            .position(|window| window == marker)
            .expect("theme line present");
        for (offset, byte) in b"256".iter().enumerate() {
            buffer[start + 9 + offset] = *byte;
        }
        assert_eq!(&buffer[start..start + 12], b"p1.theme=256");
        assert!(parse_config_text(&buffer[..len]).is_none());
    }

    #[test]
    fn stored_profiles_survive_codec_with_themes_and_fields() {
        let mut profiles = DEFAULT_PROFILES;
        profiles[0].theme_index = 2;
        profiles[2].theme_index = 0;
        let mut buffer = [0u8; STORE_FILE_MAX_BYTES];
        let len = write_config_text(&profiles, &mut buffer);
        assert!(len <= STORE_FILE_MAX_BYTES, "store file stays bounded");
        let parsed = parse_config_text(&buffer[..len]).expect("parse");
        assert_eq!(parsed, profiles);
    }

    #[test]
    fn config_text_rejects_malformed_lines() {
        let mut buffer = [0u8; 256];
        let len = write_config_text(&DEFAULT_PROFILES, &mut buffer);
        // Corrupt one line: drop the '=' from the first key.
        let mut corrupted = buffer;
        let equals = corrupted[..len]
            .iter()
            .position(|byte| *byte == b'=')
            .unwrap();
        corrupted[equals] = b'_';
        assert!(parse_config_text(&corrupted[..len]).is_none());
        // Unknown field name.
        let mut unknown = buffer;
        unknown[2] = b'x';
        assert!(parse_config_text(&unknown[..len]).is_none());
    }
}
