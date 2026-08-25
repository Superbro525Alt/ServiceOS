use rt::{LogDomain, LogEvent, LogSeverity, RuntimeEnvState, RuntimeKind};
use serviceos_userspace_runtime as rt;

use crate::{
    consts::{MAX_PROFILE_BYTES, MAX_STORAGE_PATH},
    types::{EnvSlot, FixedBytes, Profile, RunSlot},
};

pub(crate) fn emit_log(
    log_handle: rt::Handle,
    severity: LogSeverity,
    event: LogEvent,
    arg0: u64,
    arg1: u64,
) -> rt::Result<()> {
    rt::send_log_record(
        log_handle,
        rt::ServiceId::Runtime,
        severity,
        LogDomain::Runtime,
        event,
        arg0,
        arg1,
    )
}

pub(crate) fn read_profile(profile_handle: rt::Handle) -> rt::Result<Profile> {
    let mut bytes = [0u8; MAX_PROFILE_BYTES];
    let max_len = bytes.len();
    let loaded = rt::storage_read_all(profile_handle, &mut bytes, max_len)?;
    let _ = rt::storage_blob_close(profile_handle);
    let text = core::str::from_utf8(&bytes[..loaded]).map_err(|_| rt::Error::InvalidArgument)?;
    parse_profile(text)
}

fn parse_profile(text: &str) -> rt::Result<Profile> {
    let mut profile = Profile::empty();
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(rt::Error::InvalidArgument);
        };
        match key.trim() {
            "kind" => {
                profile.kind = match value.trim() {
                    "posix" => RuntimeKind::Posix,
                    "windows" => RuntimeKind::Windows,
                    _ => return Err(rt::Error::InvalidArgument),
                };
            }
            "caps" => {
                let mut caps = 0u32;
                for entry in value
                    .split(',')
                    .map(str::trim)
                    .filter(|entry| !entry.is_empty())
                {
                    caps |= match entry {
                        "file-read" => rt::runtime_capability::FILE_READ,
                        "terminal-io" => rt::runtime_capability::TERMINAL_IO,
                        "network" => rt::runtime_capability::NETWORK,
                        "graphics" => rt::runtime_capability::GRAPHICS,
                        "audio" => rt::runtime_capability::AUDIO,
                        _ => return Err(rt::Error::InvalidArgument),
                    };
                }
                profile.capabilities = caps;
            }
            "mount" => {
                if profile.mount_count == profile.mounts.len() {
                    return Err(rt::Error::CapacityExceeded);
                }
                let Some((guest, source)) = value.split_once('=') else {
                    return Err(rt::Error::InvalidArgument);
                };
                let slot = &mut profile.mounts[profile.mount_count];
                slot.guest.set(guest.trim().as_bytes())?;
                slot.source.set(source.trim().as_bytes())?;
                profile.mount_count += 1;
            }
            "env" => {
                if profile.var_count == profile.vars.len() {
                    return Err(rt::Error::CapacityExceeded);
                }
                let Some((name, value)) = value.split_once('=') else {
                    return Err(rt::Error::InvalidArgument);
                };
                let slot = &mut profile.vars[profile.var_count];
                slot.key.set(name.trim().as_bytes())?;
                slot.value.set(value.trim().as_bytes())?;
                profile.var_count += 1;
            }
            "lib" => {
                // Bundled guest library: `lib = <name> = <guest-path>` maps a
                // library name to a mounted path holding its flat image.
                if profile.lib_count == profile.libs.len() {
                    return Err(rt::Error::CapacityExceeded);
                }
                let Some((name, guest)) = value.split_once('=') else {
                    return Err(rt::Error::InvalidArgument);
                };
                let name = name.trim();
                let guest = guest.trim();
                if name.is_empty() || !guest.starts_with('/') {
                    return Err(rt::Error::InvalidArgument);
                }
                let slot = &mut profile.libs[profile.lib_count];
                slot.name.set(name.as_bytes())?;
                slot.guest.set(guest.as_bytes())?;
                profile.lib_count += 1;
            }
            _ => return Err(rt::Error::InvalidArgument),
        }
    }
    Ok(profile)
}

pub(crate) fn instantiate_env(profile: Profile) -> EnvSlot {
    let mut env = EnvSlot::empty();
    env.occupied = true;
    env.kind = profile.kind;
    env.state = RuntimeEnvState::Ready;
    env.capabilities = profile.capabilities;
    env.granted_caps = 0;
    env.sandbox = crate::sandbox::SandboxProfile::from_masks(profile.capabilities, 0);
    if env.sandbox.has_pending_classes() {
        env.state = RuntimeEnvState::PendingApproval;
    }
    env.mount_count = profile.mount_count;
    env.var_count = profile.var_count;
    env.lib_count = profile.lib_count;
    env.active_runs = 0;
    let mut index = 0usize;
    while index < profile.mount_count {
        env.mounts[index] = profile.mounts[index];
        index += 1;
    }
    index = 0;
    while index < profile.var_count {
        env.vars[index] = profile.vars[index];
        index += 1;
    }
    index = 0;
    while index < profile.lib_count {
        env.libs[index] = profile.libs[index];
        index += 1;
    }
    env
}

pub(crate) fn sensitive_capabilities(bits: u32) -> u32 {
    bits & (rt::runtime_capability::NETWORK
        | rt::runtime_capability::GRAPHICS
        | rt::runtime_capability::AUDIO)
}

pub(crate) fn release_run_slot(run: &mut RunSlot) {
    if run.task_handle != rt::INVALID_HANDLE {
        let _ = rt::handle_close(run.task_handle);
    }
    if run.session_handle != rt::INVALID_HANDLE {
        let _ = rt::handle_close(run.session_handle);
    }
    *run = RunSlot::empty();
}

pub(crate) fn resolve_guest_path(
    env: &EnvSlot,
    guest_path: &[u8],
    resolved: &mut FixedBytes<MAX_STORAGE_PATH>,
) -> rt::Result<()> {
    if guest_path.is_empty() || guest_path[0] != b'/' || contains_parent_segment(guest_path) {
        return Err(rt::Error::InvalidArgument);
    }

    let mut best_match = None;
    let mut best_len = 0usize;
    for index in 0..env.mount_count {
        let guest = env.mounts[index].guest.as_bytes();
        if matches_guest_prefix(guest_path, guest) && guest.len() >= best_len {
            best_match = Some(env.mounts[index]);
            best_len = guest.len();
        }
    }
    let Some(mount) = best_match else {
        return Err(rt::Error::NotFound);
    };

    let suffix = &guest_path[best_len..];
    let mut bytes = [0u8; MAX_STORAGE_PATH];
    let source = mount.source.as_bytes();
    if source.len() > bytes.len() {
        return Err(rt::Error::BufferTooSmall);
    }
    bytes[..source.len()].copy_from_slice(source);
    let mut len = source.len();
    if !suffix.is_empty() {
        if len + suffix.len() > bytes.len() {
            return Err(rt::Error::BufferTooSmall);
        }
        bytes[len..len + suffix.len()].copy_from_slice(suffix);
        len += suffix.len();
    }
    resolved.set(&bytes[..len])
}

fn contains_parent_segment(path: &[u8]) -> bool {
    path.windows(2).any(|window| window == b"//")
        || path == b"/.."
        || path.starts_with(b"../")
        || path.ends_with(b"/..")
        || path.windows(4).any(|window| window == b"/../")
}

fn matches_guest_prefix(path: &[u8], prefix: &[u8]) -> bool {
    path.starts_with(prefix)
        && (path.len() == prefix.len() || path.get(prefix.len()) == Some(&b'/'))
}

pub(crate) fn pack_pair(first: &[u8], second: &[u8], words: &mut [u64]) -> rt::Result<u32> {
    let mut combined = [0u8; rt::IPC_MAX_WORDS * 8];
    if first.len() + second.len() > combined.len() {
        return Err(rt::Error::BufferTooSmall);
    }
    combined[..first.len()].copy_from_slice(first);
    combined[first.len()..first.len() + second.len()].copy_from_slice(second);
    rt::pack_bytes(&combined[..first.len() + second.len()], words)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consts::MAX_LIBS;

    #[test]
    fn descriptor_parses_bundled_lib_lines_into_profile() {
        let text = "kind=posix\ncaps=file-read\nlib = libc = /lib/libc.so.sosimg\nlib=libm=/lib/libm.so.sosimg\n";
        let profile = parse_profile(text).expect("profile");
        assert_eq!(profile.lib_count, 2);
        assert_eq!(profile.libs[0].name.as_bytes(), b"libc");
        assert_eq!(profile.libs[0].guest.as_bytes(), b"/lib/libc.so.sosimg");
        assert_eq!(profile.libs[1].name.as_bytes(), b"libm");
    }

    #[test]
    fn descriptor_rejects_malformed_lib_lines_and_overflow() {
        assert!(parse_profile("lib = nopath").is_err());
        assert!(parse_profile("lib = name = relative/path").is_err());
        let mut overflowed = String::from("kind=posix\n");
        for index in 0..MAX_LIBS + 1 {
            use core::fmt::Write as _;
            let _ = write!(overflowed, "lib = l{index} = /lib/l{index}\n");
        }
        assert!(matches!(
            parse_profile(&overflowed),
            Err(rt::Error::CapacityExceeded)
        ));
    }

    #[test]
    fn instantiate_env_carries_declared_libs_into_env_record() {
        let profile =
            parse_profile("kind=posix\nlib = libc = /lib/libc.so.sosimg\n").expect("profile");
        let env = instantiate_env(profile);
        assert_eq!(env.lib_count, 1);
        assert_eq!(env.libs[0].name.as_bytes(), b"libc");
        assert_eq!(env.libs[0].guest.as_bytes(), b"/lib/libc.so.sosimg");
        assert_eq!(EnvSlot::empty().lib_count, 0);
    }
}
