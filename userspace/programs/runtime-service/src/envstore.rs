//! Cross-reboot persistence for runtime environment records (roadmap row
//! 169 residual). House pattern, service-local: a keystore-codec cfg file
//! under `state/runtime/` capturing each occupied `EnvSlot` additively
//! (kind, lifecycle state, capabilities, granted subset, syscall-ABI mode,
//! sandbox class masks, mounts, vars, bundled libs, latched workload
//! manifest, boot-local tick stamps) plus the per-env-kind policy defaults
//! (additive `policy` section; see `policy.rs`). Write-through happens on
//! the mutating ops (create / env-decision / run-launch manifest latch /
//! destroy / policy set) via a before/after record snapshot in the
//! dispatcher; startup rehydrates the table so env ids (slot indices) stay
//! stable across restarts.
//!
//! Honesty contracts:
//! - live-only state is never persisted: `active_runs` resets to 0 on
//!   rehydrate, and non-Denied lifecycle states are re-derived from the
//!   persisted capability masks (`Denied` is an operator verdict and
//!   survives verbatim).
//! - corrupt store lines are skipped and counted; when any line is corrupt
//!   the store is rehydrated where possible but write-through stays
//!   DISABLED for the boot (account-service precedent: never clobber
//!   evidence).
//! - fresh boots with no records log nothing and write nothing.
//!
//! Env ids are slot indices (house contract), so stability across restarts
//! comes from rehydrating records back into their persisted slots.

use crate::{
    consts::{
        MAX_ENVS, MAX_GUEST_PATH, MAX_LIBS, MAX_MOUNTS, MAX_STORAGE_PATH, MAX_VAR_KEY,
        MAX_VAR_VALUE, MAX_VARS,
    },
    policy::{EnvPolicyDefault, PolicyTable},
    sandbox::{
        CLASS_COUNT, DEVICE_CLASSES, SANDBOX_MANIFEST_VERSION, SandboxManifest, SandboxProfile,
    },
    types::{EnvSlot, FixedBytes, LibSlot, MountSlot, VarSlot},
    util::sensitive_capabilities,
};
use serviceos_userspace_runtime as rt;

/// Service-local durable state namespace (house pattern: `state/<service>/`).
pub(crate) const ENVSTORE_DIR: &str = "state/runtime";
pub(crate) const ENVSTORE_FILE: &str = "environments.cfg";
pub(crate) const ENVSTORE_PATH: &str = "state/runtime/environments.cfg";

/// Upper bound for the serialized store. Four fully-packed environments
/// occupy ~3 KB; the headroom keeps growth from truncating silently.
const MAX_ENVSTORE_BYTES: usize = 8192;

/// Store grammar marker. A file whose first non-empty line differs is not
/// accepted (foreign or future format); nothing is rehydrated and the
/// corrupt-line counter makes the rejection loud.
const ENVSTORE_MAGIC: &str = "runtime-envs1";

/// Startup rehydrate outcome. `Empty` allows write-through; `Corrupt` means
/// the store existed but was not cleanly parsed, so this boot must not
/// rewrite it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RehydrateOutcome {
    /// No persisted store (first boot or empty file).
    Empty,
    /// N records rehydrated cleanly.
    Rehydrated(usize),
    /// Records rehydrated where possible, N lines skipped as corrupt;
    /// persistence is disabled for this boot.
    Corrupt(usize),
}

/// Serialize the occupied env records into `buffer` (keystore-codec style:
/// one magic line, then `policy` section lines for configured env-kind
/// defaults, then `env` header lines with `mount`/`var`/`lib`/`manifest`
/// detail lines). Returns the byte length written.
pub(crate) fn format_envs(
    envs: &[EnvSlot; MAX_ENVS],
    policy: &PolicyTable,
    buffer: &mut [u8],
) -> Result<usize, rt::Error> {
    let mut cursor = FormatCursor::new(buffer);
    cursor.push_str(ENVSTORE_MAGIC);
    cursor.push(b'\n');
    // Additive policy section: only configured (non-Ask) kinds are written,
    // so a default-state table serializes byte-identically to the pre-policy
    // grammar and fresh boots write nothing.
    for kind in PolicyTable::kinds() {
        let default = policy.default_for(kind);
        if matches!(default, EnvPolicyDefault::Ask) {
            continue;
        }
        cursor.push_str("policy ");
        cursor.push_decimal(kind as u32 as u64);
        cursor.push(b' ');
        cursor.push_decimal(default.word());
        cursor.push(b'\n');
    }
    for (id, env) in envs.iter().enumerate() {
        if !env.occupied {
            continue;
        }
        cursor.push_str("env ");
        cursor.push_decimal(id as u64);
        cursor.push(b' ');
        cursor.push_decimal(env.kind as u32 as u64);
        cursor.push(b' ');
        cursor.push_decimal(env.state as u32 as u64);
        cursor.push(b' ');
        cursor.push_hex_fixed(env.capabilities as u64, 8);
        cursor.push(b' ');
        cursor.push_hex_fixed(env.granted_caps as u64, 8);
        cursor.push(b' ');
        cursor.push_decimal(env.linux_syscall as u64);
        cursor.push(b' ');
        cursor.push_hex_fixed(env.sandbox.requested_mask() as u64, 2);
        cursor.push(b' ');
        cursor.push_hex_fixed(env.sandbox.granted_mask() as u64, 2);
        cursor.push(b' ');
        cursor.push_decimal(env.created_tick);
        cursor.push(b' ');
        cursor.push_decimal(env.updated_tick);
        cursor.push(b'\n');
        for index in 0..env.mount_count {
            cursor.push_str("mount ");
            cursor.push_hex_bytes(env.mounts[index].guest.as_bytes());
            cursor.push(b' ');
            cursor.push_hex_bytes(env.mounts[index].source.as_bytes());
            cursor.push(b'\n');
        }
        for index in 0..env.var_count {
            cursor.push_str("var ");
            cursor.push_hex_bytes(env.vars[index].key.as_bytes());
            cursor.push(b' ');
            cursor.push_hex_bytes(env.vars[index].value.as_bytes());
            cursor.push(b'\n');
        }
        for index in 0..env.lib_count {
            cursor.push_str("lib ");
            cursor.push_hex_bytes(env.libs[index].name.as_bytes());
            cursor.push(b' ');
            cursor.push_hex_bytes(env.libs[index].guest.as_bytes());
            cursor.push(b'\n');
        }
        if let Some(manifest) = env.manifest {
            cursor.push_str("manifest ");
            cursor.push_decimal(manifest.version as u64);
            cursor.push(b' ');
            cursor.push_hex_fixed(manifest.grants_mask() as u64, 2);
            cursor.push(b' ');
            match manifest.caps_allow {
                Some(mask) => cursor.push_hex_fixed(mask as u64, 8),
                None => cursor.push(b'-'),
            }
            cursor.push(b'\n');
        }
    }
    cursor.finish()
}

/// Rehydrate env records and the policy table from previously formatted
/// store text. Corrupt lines are skipped and counted (keystore precedent: a
/// partial write must never lock every record out). Returns
/// (rehydrated, corrupt_lines).
pub(crate) fn parse_envs(
    text: &str,
    envs: &mut [EnvSlot; MAX_ENVS],
    policy: &mut PolicyTable,
) -> (usize, usize) {
    let mut rehydrated = 0usize;
    let mut corrupt = 0usize;
    let mut magic_seen = false;
    let mut policy_seen = [false; 2];
    // Index of the env record the current detail lines attach to.
    let mut current: Option<usize> = None;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if !magic_seen {
            if line == ENVSTORE_MAGIC {
                magic_seen = true;
            } else {
                corrupt += 1;
            }
            continue;
        }
        // Top-level policy section lines: kind defaults are global state, so
        // unlike mount/var/lib/manifest details they do not attach to the
        // current env record. Duplicate kind lines are corrupt (an ambiguous
        // store must not be rewritten).
        if let Some(rest) = line.strip_prefix("policy ") {
            corrupt += apply_policy(policy, &mut policy_seen, rest);
            continue;
        }
        if let Some(rest) = line.strip_prefix("env ") {
            match parse_env_header(rest, envs) {
                Some(id) => {
                    current = Some(id);
                    rehydrated += 1;
                }
                None => {
                    current = None;
                    corrupt += 1;
                }
            }
            continue;
        }
        let Some(id) = current else {
            corrupt += 1;
            continue;
        };
        let env = &mut envs[id];
        if let Some(rest) = line.strip_prefix("mount ") {
            corrupt += apply_mount(env, rest);
        } else if let Some(rest) = line.strip_prefix("var ") {
            corrupt += apply_var(env, rest);
        } else if let Some(rest) = line.strip_prefix("lib ") {
            corrupt += apply_lib(env, rest);
        } else if let Some(rest) = line.strip_prefix("manifest ") {
            corrupt += apply_manifest(env, rest);
        } else {
            corrupt += 1;
        }
    }
    if !magic_seen {
        // No header: nothing may be rehydrated regardless of line counts.
        return (0, corrupt);
    }
    (rehydrated, corrupt)
}

/// Load the persisted env table at startup. A missing store (`NotFound`) or
/// an empty file is a fresh first boot; an unreadable or corrupt store
/// reports `Corrupt` so the caller disables write-through instead of
/// clobbering evidence.
pub(crate) fn load_envs(
    storage_handle: rt::Handle,
    envs: &mut [EnvSlot; MAX_ENVS],
    policy: &mut PolicyTable,
) -> RehydrateOutcome {
    let (blob, len) = match rt::storage_open(storage_handle, ENVSTORE_PATH) {
        Ok(opened) => opened,
        Err(rt::Error::NotFound) => return RehydrateOutcome::Empty,
        Err(_) => return RehydrateOutcome::Corrupt(1),
    };
    let len = len.min(MAX_ENVSTORE_BYTES);
    let mut bytes = [0u8; MAX_ENVSTORE_BYTES];
    let mut offset = 0usize;
    let mut read_ok = true;
    while offset < len {
        match rt::storage_read(blob, offset, &mut bytes[offset..len]) {
            Ok(0) => break,
            Ok(read) => offset += read,
            Err(_) => {
                read_ok = false;
                break;
            }
        }
    }
    let _ = rt::storage_blob_close(blob);
    if !read_ok {
        return RehydrateOutcome::Corrupt(1);
    }
    if bytes[..offset].iter().all(|&byte| byte == 0) {
        return RehydrateOutcome::Empty;
    }
    let Ok(text) = core::str::from_utf8(&bytes[..offset]) else {
        return RehydrateOutcome::Corrupt(1);
    };
    let text = text.trim_end_matches('\0');
    let (rehydrated, corrupt) = parse_envs(text, envs, policy);
    if corrupt != 0 {
        RehydrateOutcome::Corrupt(corrupt)
    } else if rehydrated == 0 {
        RehydrateOutcome::Empty
    } else {
        RehydrateOutcome::Rehydrated(rehydrated)
    }
}

/// Write-through: serialize the table and rewrite the store file in full
/// (the storage write path truncates to the written length). Silent on
/// storage failure, matching the account-service persist precedent.
pub(crate) fn persist_envs(
    storage_handle: rt::Handle,
    envs: &[EnvSlot; MAX_ENVS],
    policy: &PolicyTable,
) {
    let Ok(dir) = ensure_envstore_dir(storage_handle) else {
        return;
    };
    let (file, _) = match rt::storage_directory_open_file(dir, ENVSTORE_FILE, true, true) {
        Ok(opened) => opened,
        Err(_) => {
            let _ = rt::handle_close(dir);
            return;
        }
    };
    let mut buffer = [0u8; MAX_ENVSTORE_BYTES];
    if let Ok(total) = format_envs(envs, policy, &mut buffer) {
        let mut offset = 0usize;
        while offset < total {
            let chunk_len = (total - offset).min((rt::IPC_MAX_WORDS - 3) * 8);
            if rt::storage_write(file, offset, total, &buffer[offset..offset + chunk_len]).is_err()
            {
                break;
            }
            offset += chunk_len;
        }
    }
    let _ = rt::storage_blob_close(file);
    let _ = rt::handle_close(dir);
}

fn ensure_envstore_dir(storage_handle: rt::Handle) -> Result<rt::Handle, rt::Error> {
    if let Ok(dir) = rt::storage_open_directory(storage_handle, ENVSTORE_DIR, true) {
        return Ok(dir);
    }
    let state = rt::storage_open_directory(storage_handle, "state", true)?;
    let created = rt::storage_directory_create(state, "runtime", rt::StorageEntryKind::Directory);
    let _ = rt::handle_close(state);
    created?;
    rt::storage_open_directory(storage_handle, ENVSTORE_DIR, true)
}

// ---- line parsers -------------------------------------------------------

/// `"<kind> <default>"` after the `policy ` prefix (kind word 1 = posix,
/// 2 = windows; default word 0 = ask, 1 = allow-all, 2 = deny-all). Only
/// non-Ask kinds are ever serialized, but Ask parses fine (idempotent
/// re-assertion of the default). Returns 1 when the line was dropped as
/// corrupt, else 0.
fn apply_policy(policy: &mut PolicyTable, policy_seen: &mut [bool; 2], rest: &str) -> usize {
    let mut fields = rest.split(' ');
    let (Some(kind_word), Some(default_word), None) = (fields.next(), fields.next(), fields.next())
    else {
        return 1;
    };
    let (Some(kind_word), Some(default_word)) =
        (parse_u64(kind_word).ok(), parse_u64(default_word).ok())
    else {
        return 1;
    };
    let kind = match kind_word as u32 {
        x if x == rt::RuntimeKind::Posix as u32 => rt::RuntimeKind::Posix,
        x if x == rt::RuntimeKind::Windows as u32 => rt::RuntimeKind::Windows,
        _ => return 1,
    };
    let index = kind as u32 as usize - 1;
    if policy_seen[index] {
        return 1;
    }
    match crate::policy::EnvPolicyDefault::from_word(default_word) {
        Some(default) => {
            policy_seen[index] = true;
            policy.set(kind, default);
            0
        }
        None => 1,
    }
}

/// `"<id> <kind> <state> <caps> <granted> <linux> <req> <grt> <created>
/// <updated>"` after the `env ` prefix. Rebuilds the record with live state
/// reset honestly: `active_runs` starts at zero and non-Denied lifecycle
/// states are re-derived from the persisted capability masks, so a stale or
/// hand-edited state word can never widen a grant.
fn parse_env_header(rest: &str, envs: &mut [EnvSlot; MAX_ENVS]) -> Option<usize> {
    let mut fields = rest.split(' ');
    let id = parse_usize(fields.next()?)?;
    let kind_word = parse_u32(fields.next()?).ok()?;
    let state_word = parse_u32(fields.next()?).ok()?;
    let capabilities = parse_u32_hex(fields.next()?).ok()?;
    let granted_caps = parse_u32_hex(fields.next()?).ok()?;
    let linux_syscall = parse_bool(fields.next()?).ok()?;
    let requested_mask = parse_u32_hex(fields.next()?).ok()?;
    let granted_mask = parse_u32_hex(fields.next()?).ok()?;
    let created_tick = parse_u64(fields.next()?).ok()?;
    let updated_tick = parse_u64(fields.next()?).ok()?;
    if fields.next().is_some() || id >= MAX_ENVS || envs[id].occupied {
        // Trailing junk or a duplicate record for the same slot.
        return None;
    }
    let kind = match kind_word {
        x if x == rt::RuntimeKind::Posix as u32 => rt::RuntimeKind::Posix,
        x if x == rt::RuntimeKind::Windows as u32 => rt::RuntimeKind::Windows,
        _ => return None,
    };
    // Only durable lifecycle states may appear in the store: `Denied` is an
    // operator verdict that survives verbatim; Ready/PendingApproval are
    // re-derived from the capability masks on load.
    let persisted_denied = state_word == rt::RuntimeEnvState::Denied as u32;
    let durable_state = state_word == rt::RuntimeEnvState::Ready as u32
        || state_word == rt::RuntimeEnvState::PendingApproval as u32;
    if !persisted_denied && !durable_state {
        return None;
    }
    let mut env = EnvSlot::empty();
    env.occupied = true;
    env.kind = kind;
    env.capabilities = capabilities;
    env.granted_caps = granted_caps;
    env.linux_syscall = linux_syscall;
    env.sandbox = profile_from_masks(requested_mask, granted_mask);
    env.state = if persisted_denied {
        rt::RuntimeEnvState::Denied
    } else if sensitive_capabilities(capabilities) & !granted_caps != 0 {
        rt::RuntimeEnvState::PendingApproval
    } else {
        rt::RuntimeEnvState::Ready
    };
    env.created_tick = created_tick;
    env.updated_tick = updated_tick;
    envs[id] = env;
    Some(id)
}

/// Rebuild the class matrix from persisted requested/granted class masks
/// (exact reconstruction; no capability-bit mapping assumptions).
fn profile_from_masks(requested_mask: u32, granted_mask: u32) -> SandboxProfile {
    let mut profile = SandboxProfile::empty();
    for class in DEVICE_CLASSES {
        if requested_mask & (1 << class.index()) != 0 {
            profile.request(class);
        }
        if granted_mask & (1 << class.index()) != 0 {
            profile.grant(class);
        }
    }
    profile
}

/// Returns 1 when the detail line was dropped as corrupt, else 0.
fn apply_mount(env: &mut EnvSlot, rest: &str) -> usize {
    let mut fields = rest.split(' ');
    let (Some(guest), Some(source), None) = (fields.next(), fields.next(), fields.next()) else {
        return 1;
    };
    let (Some(guest), Some(source)) = (
        hex_fixed::<MAX_GUEST_PATH>(guest),
        hex_fixed::<MAX_STORAGE_PATH>(source),
    ) else {
        return 1;
    };
    if env.mount_count >= MAX_MOUNTS {
        return 1;
    }
    env.mounts[env.mount_count] = MountSlot { guest, source };
    env.mount_count += 1;
    0
}

fn apply_var(env: &mut EnvSlot, rest: &str) -> usize {
    let mut fields = rest.split(' ');
    let (Some(key), Some(value), None) = (fields.next(), fields.next(), fields.next()) else {
        return 1;
    };
    let (Some(key), Some(value)) = (
        hex_fixed::<MAX_VAR_KEY>(key),
        hex_fixed::<MAX_VAR_VALUE>(value),
    ) else {
        return 1;
    };
    if env.var_count >= MAX_VARS {
        return 1;
    }
    env.vars[env.var_count] = VarSlot { key, value };
    env.var_count += 1;
    0
}

fn apply_lib(env: &mut EnvSlot, rest: &str) -> usize {
    let mut fields = rest.split(' ');
    let (Some(name), Some(guest), None) = (fields.next(), fields.next(), fields.next()) else {
        return 1;
    };
    let (Some(name), Some(guest)) = (
        hex_fixed::<MAX_VAR_KEY>(name),
        hex_fixed::<MAX_GUEST_PATH>(guest),
    ) else {
        return 1;
    };
    if env.lib_count >= MAX_LIBS {
        return 1;
    }
    env.libs[env.lib_count] = LibSlot { name, guest };
    env.lib_count += 1;
    0
}

/// `"<version> <grants-hex> <capsallow: 8hex | ->"`. The version must match
/// this build's wire manifest version; anything else is corrupt (the launch
/// gate refuses version mismatches loudly, and so does the store).
fn apply_manifest(env: &mut EnvSlot, rest: &str) -> usize {
    let mut fields = rest.split(' ');
    let (Some(version), Some(grants), Some(caps_allow), None) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return 1;
    };
    let Ok(version) = parse_u32(version) else {
        return 1;
    };
    if version != u32::from(SANDBOX_MANIFEST_VERSION) {
        return 1;
    }
    let Ok(grants_mask) = parse_u32_hex(grants) else {
        return 1;
    };
    let caps_allow = if caps_allow == "-" {
        None
    } else {
        match parse_u32_hex(caps_allow) {
            Ok(mask) => Some(mask),
            Err(_) => return 1,
        }
    };
    if env.manifest.is_some() {
        return 1;
    }
    let mut grants = [false; CLASS_COUNT];
    for (index, slot) in grants.iter_mut().enumerate() {
        *slot = grants_mask & (1 << index) != 0;
    }
    env.manifest = Some(SandboxManifest {
        version: version as u8,
        grants,
        caps_allow,
    });
    0
}

// ---- scalar parsers -----------------------------------------------------

fn parse_usize(text: &str) -> Option<usize> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse::<usize>().ok()
}

fn parse_u32(text: &str) -> Result<u32, ()> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(());
    }
    text.parse::<u32>().map_err(|_| ())
}

fn parse_u64(text: &str) -> Result<u64, ()> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(());
    }
    text.parse::<u64>().map_err(|_| ())
}

/// Fixed-width (8-digit) lowercase hex u32.
fn parse_u32_hex(text: &str) -> Result<u32, ()> {
    if text.is_empty() || text.len() > 8 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(());
    }
    u32::from_str_radix(text, 16).map_err(|_| ())
}

fn parse_bool(text: &str) -> Result<bool, ()> {
    match text {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(()),
    }
}

/// Decode a lowercase hex string into a `FixedBytes<N>`; oversize, odd
/// length, empty, or non-hex input fails.
fn hex_fixed<const N: usize>(text: &str) -> Option<FixedBytes<N>> {
    let bytes = text.as_bytes();
    if bytes.is_empty() || bytes.len() % 2 != 0 || bytes.len() / 2 > N {
        return None;
    }
    let mut slot = FixedBytes::<N>::empty();
    for pair in bytes.chunks(2) {
        let hi = hex_nibble(pair[0])?;
        let lo = hex_nibble(pair[1])?;
        slot.bytes[slot.len] = (hi << 4) | lo;
        slot.len += 1;
    }
    Some(slot)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

// ---- formatting cursor --------------------------------------------------

/// Bounded no-alloc text builder (house FormatBuf pattern): any overflow
/// marks the whole serialization as failed instead of truncating silently.
struct FormatCursor<'a> {
    buffer: &'a mut [u8],
    len: usize,
    overflow: bool,
}

impl<'a> FormatCursor<'a> {
    fn new(buffer: &'a mut [u8]) -> Self {
        Self {
            buffer,
            len: 0,
            overflow: false,
        }
    }

    fn push(&mut self, byte: u8) {
        if self.len < self.buffer.len() {
            self.buffer[self.len] = byte;
            self.len += 1;
        } else {
            self.overflow = true;
        }
    }

    fn push_str(&mut self, text: &str) {
        for &byte in text.as_bytes() {
            self.push(byte);
        }
    }

    fn push_decimal(&mut self, mut value: u64) {
        if value == 0 {
            self.push(b'0');
            return;
        }
        let mut digits = [0u8; 20];
        let mut count = 0usize;
        while value != 0 {
            digits[count] = b'0' + (value % 10) as u8;
            value /= 10;
            count += 1;
        }
        while count > 0 {
            count -= 1;
            self.push(digits[count]);
        }
    }

    /// Lowercase hex, zero-padded to `digits` nibbles (`digits` <= 16).
    fn push_hex_fixed(&mut self, value: u64, digits: usize) {
        let mut index = digits;
        while index > 0 {
            index -= 1;
            let nibble = ((value >> (index * 4)) & 0xf) as u8;
            self.push(if nibble < 10 {
                b'0' + nibble
            } else {
                b'a' + nibble - 10
            });
        }
    }

    fn push_hex_bytes(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            let high = byte >> 4;
            let low = byte & 0xf;
            self.push(if high < 10 {
                b'0' + high
            } else {
                b'a' + high - 10
            });
            self.push(if low < 10 {
                b'0' + low
            } else {
                b'a' + low - 10
            });
        }
    }

    fn finish(self) -> Result<usize, rt::Error> {
        if self.overflow {
            Err(rt::Error::BufferTooSmall)
        } else {
            Ok(self.len)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        consts::{MAX_GUEST_PATH, MAX_STORAGE_PATH, MAX_VAR_KEY, MAX_VAR_VALUE},
        protocol::{apply_decision, envs::allocate_env},
        sandbox::SANDBOX_MANIFEST_VERSION,
        types::{LibSlot, Profile},
        util::instantiate_env,
    };
    use rt::{PermissionPolicyState, RuntimeEnvState};

    fn fixed<const N: usize>(value: &[u8]) -> FixedBytes<N> {
        let mut slot = FixedBytes::<N>::empty();
        slot.set(value).unwrap();
        slot
    }

    fn sample_profile() -> Profile {
        let mut profile = Profile {
            kind: rt::RuntimeKind::Posix,
            capabilities: rt::runtime_capability::FILE_READ | rt::runtime_capability::TERMINAL_IO,
            requested_caps: rt::runtime_capability::NETWORK,
            linux_syscall: true,
            mounts: [MountSlot::empty(); MAX_MOUNTS],
            mount_count: 2,
            vars: [VarSlot::empty(); MAX_VARS],
            var_count: 2,
            libs: [LibSlot::empty(); MAX_LIBS],
            lib_count: 1,
        };
        profile.mounts[0] = MountSlot {
            guest: fixed::<MAX_GUEST_PATH>(b"/data"),
            source: fixed::<MAX_STORAGE_PATH>(b"state/data"),
        };
        profile.mounts[1] = MountSlot {
            guest: fixed::<MAX_GUEST_PATH>(b"/tmp,a,b c"),
            source: fixed::<MAX_STORAGE_PATH>(b"tmpfs:scratch/x"),
        };
        profile.vars[0] = VarSlot {
            key: fixed::<MAX_VAR_KEY>(b"PATH"),
            value: fixed::<MAX_VAR_VALUE>(b"/bin,/usr/bin"),
        };
        profile.vars[1] = VarSlot {
            key: fixed::<MAX_VAR_KEY>(b"spaced key"),
            value: fixed::<MAX_VAR_VALUE>(b"with spaces  and , commas"),
        };
        profile.libs[0] = LibSlot {
            name: fixed::<MAX_VAR_KEY>(b"libc.so"),
            guest: fixed::<MAX_GUEST_PATH>(b"/libs/libc.img"),
        };
        profile
    }

    fn manifest(caps_allow: Option<u32>) -> SandboxManifest {
        SandboxManifest {
            version: SANDBOX_MANIFEST_VERSION,
            grants: [true, false, true, false],
            caps_allow,
        }
    }

    fn assert_env_equal(left: &EnvSlot, right: &EnvSlot) {
        assert_eq!(left.kind, right.kind);
        assert_eq!(left.state, right.state);
        assert_eq!(left.capabilities, right.capabilities);
        assert_eq!(left.granted_caps, right.granted_caps);
        assert_eq!(left.linux_syscall, right.linux_syscall);
        assert_eq!(
            left.sandbox.requested_mask(),
            right.sandbox.requested_mask()
        );
        assert_eq!(left.sandbox.granted_mask(), right.sandbox.granted_mask());
        assert_eq!(left.mount_count, right.mount_count);
        assert_eq!(left.var_count, right.var_count);
        assert_eq!(left.lib_count, right.lib_count);
        assert_eq!(left.active_runs, right.active_runs);
        assert_eq!(left.manifest, right.manifest);
        assert_eq!(left.created_tick, right.created_tick);
        assert_eq!(left.updated_tick, right.updated_tick);
        for index in 0..left.mount_count {
            assert_eq!(
                left.mounts[index].guest.as_bytes(),
                right.mounts[index].guest.as_bytes()
            );
            assert_eq!(
                left.mounts[index].source.as_bytes(),
                right.mounts[index].source.as_bytes()
            );
        }
        for index in 0..left.var_count {
            assert_eq!(
                left.vars[index].key.as_bytes(),
                right.vars[index].key.as_bytes()
            );
            assert_eq!(
                left.vars[index].value.as_bytes(),
                right.vars[index].value.as_bytes()
            );
        }
        for index in 0..left.lib_count {
            assert_eq!(
                left.libs[index].name.as_bytes(),
                right.libs[index].name.as_bytes()
            );
            assert_eq!(
                left.libs[index].guest.as_bytes(),
                right.libs[index].guest.as_bytes()
            );
        }
    }

    #[test]
    fn codec_roundtrip_full_record_including_manifest() {
        let mut envs = [EnvSlot::empty(); MAX_ENVS];
        envs[2] = instantiate_env(sample_profile());
        envs[2].granted_caps = rt::runtime_capability::NETWORK;
        envs[2].state = RuntimeEnvState::Ready;
        envs[2].sandbox.apply_granted_mask(envs[2].granted_caps);
        envs[2].manifest = Some(manifest(Some(0xff00_00ff)));
        envs[2].created_tick = 42;
        envs[2].updated_tick = 4242;

        let mut buffer = [0u8; MAX_ENVSTORE_BYTES];
        let total = format_envs(&envs, &policy_table(), &mut buffer).expect("serialize");
        let text = core::str::from_utf8(&buffer[..total]).unwrap();

        let mut rehydrated = [EnvSlot::empty(); MAX_ENVS];
        let (count, corrupt) = parse_envs(text, &mut rehydrated, &mut policy_table());
        assert_eq!((count, corrupt), (1, 0));
        assert_env_equal(&envs[2], &rehydrated[2]);
        assert!(rehydrated[0].occupied == false && rehydrated[1].occupied == false);
    }

    #[test]
    fn manifest_without_caps_allow_roundtrips() {
        let mut envs = [EnvSlot::empty(); MAX_ENVS];
        envs[0] = instantiate_env(sample_profile());
        envs[0].manifest = Some(manifest(None));
        let mut buffer = [0u8; MAX_ENVSTORE_BYTES];
        let total = format_envs(&envs, &policy_table(), &mut buffer).expect("serialize");
        let text = core::str::from_utf8(&buffer[..total]).unwrap();
        let mut rehydrated = [EnvSlot::empty(); MAX_ENVS];
        let (count, corrupt) = parse_envs(text, &mut rehydrated, &mut policy_table());
        assert_eq!((count, corrupt), (1, 0));
        assert_eq!(envs[0].manifest, rehydrated[0].manifest);
    }

    #[test]
    fn rehydrate_resets_live_run_state() {
        let mut envs = [EnvSlot::empty(); MAX_ENVS];
        envs[1] = instantiate_env(sample_profile());
        // Live-only bookkeeping never enters the store: active runs die with
        // the service, so the rehydrated record must start at zero.
        envs[1].active_runs = 2;
        let mut buffer = [0u8; MAX_ENVSTORE_BYTES];
        let total = format_envs(&envs, &policy_table(), &mut buffer).expect("serialize");
        let text = core::str::from_utf8(&buffer[..total]).unwrap();
        let mut rehydrated = [EnvSlot::empty(); MAX_ENVS];
        let (count, corrupt) = parse_envs(text, &mut rehydrated, &mut policy_table());
        assert_eq!((count, corrupt), (1, 0));
        assert_eq!(rehydrated[1].active_runs, 0);
        assert!(rehydrated[1].occupied);
    }

    #[test]
    fn rehydrate_denied_survives_verbatim() {
        let mut envs = [EnvSlot::empty(); MAX_ENVS];
        envs[3] = instantiate_env(sample_profile());
        envs[3].state = RuntimeEnvState::Denied;
        envs[3].granted_caps = 0;
        let mut buffer = [0u8; MAX_ENVSTORE_BYTES];
        let total = format_envs(&envs, &policy_table(), &mut buffer).expect("serialize");
        let text = core::str::from_utf8(&buffer[..total]).unwrap();
        let mut rehydrated = [EnvSlot::empty(); MAX_ENVS];
        let (count, corrupt) = parse_envs(text, &mut rehydrated, &mut policy_table());
        assert_eq!((count, corrupt), (1, 0));
        assert!(matches!(rehydrated[3].state, RuntimeEnvState::Denied));
    }

    #[test]
    fn rehydrate_self_heals_state_from_masks() {
        // A record stored Ready but carrying ungranted sensitive caps is
        // honestly re-derived to PendingApproval on load; the reverse (full
        // grants stored under PendingApproval) heals to Ready. Neither can
        // widen grants: granted_caps persist exactly as written.
        let mut envs = [EnvSlot::empty(); MAX_ENVS];
        envs[0] = instantiate_env(sample_profile());
        envs[0].granted_caps = 0;
        envs[0].sandbox = crate::sandbox::SandboxProfile::from_masks(envs[0].capabilities, 0);
        envs[0].state = RuntimeEnvState::Ready;
        assert!(matches!(envs[0].state, RuntimeEnvState::Ready));
        let mut buffer = [0u8; MAX_ENVSTORE_BYTES];
        let total = format_envs(&envs, &policy_table(), &mut buffer).expect("serialize");
        let text = core::str::from_utf8(&buffer[..total]).unwrap();
        let mut rehydrated = [EnvSlot::empty(); MAX_ENVS];
        let (count, corrupt) = parse_envs(text, &mut rehydrated, &mut policy_table());
        assert_eq!((count, corrupt), (1, 0));
        assert!(matches!(
            rehydrated[0].state,
            RuntimeEnvState::PendingApproval
        ));
        assert!(
            !rehydrated[0]
                .sandbox
                .class_granted(crate::sandbox::DeviceClass::Network)
        );
    }

    #[test]
    fn env_ids_stay_stable_across_roundtrip() {
        let mut envs = [EnvSlot::empty(); MAX_ENVS];
        envs[1] = instantiate_env(sample_profile());
        envs[3] = instantiate_env(sample_profile());
        let mut buffer = [0u8; MAX_ENVSTORE_BYTES];
        let total = format_envs(&envs, &policy_table(), &mut buffer).expect("serialize");
        let text = core::str::from_utf8(&buffer[..total]).unwrap();
        let mut rehydrated = [EnvSlot::empty(); MAX_ENVS];
        let (count, corrupt) = parse_envs(text, &mut rehydrated, &mut policy_table());
        assert_eq!((count, corrupt), (2, 0));
        assert!(!rehydrated[0].occupied && rehydrated[1].occupied);
        assert!(!rehydrated[2].occupied && rehydrated[3].occupied);
    }

    #[test]
    fn destroyed_records_leave_an_empty_but_valid_store() {
        let mut envs = [EnvSlot::empty(); MAX_ENVS];
        envs[0] = instantiate_env(sample_profile());
        // EnvDestroy semantics: slot emptied, next format has no records.
        envs[0] = EnvSlot::empty();
        let mut buffer = [0u8; MAX_ENVSTORE_BYTES];
        let total = format_envs(&envs, &policy_table(), &mut buffer).expect("serialize");
        let text = core::str::from_utf8(&buffer[..total]).unwrap();
        let mut rehydrated = [EnvSlot::empty(); MAX_ENVS];
        let (count, corrupt) = parse_envs(text, &mut rehydrated, &mut policy_table());
        assert_eq!((count, corrupt), (0, 0));
    }

    #[test]
    fn corrupt_lines_are_skipped_and_counted() {
        let mut envs = [EnvSlot::empty(); MAX_ENVS];
        envs[1] = instantiate_env(sample_profile());
        let mut buffer = [0u8; MAX_ENVSTORE_BYTES];
        let total = format_envs(&envs, &policy_table(), &mut buffer).expect("serialize");
        let mut text = core::str::from_utf8(&buffer[..total]).unwrap().to_string();
        text.push_str("this is not a store line\n");
        text.push_str("mount zz\n");
        let mut rehydrated = [EnvSlot::empty(); MAX_ENVS];
        let (count, corrupt) = parse_envs(&text, &mut rehydrated, &mut policy_table());
        assert_eq!((count, corrupt), (1, 2));
        assert_eq!(rehydrated[1].mount_count, envs[1].mount_count);
    }

    #[test]
    fn missing_magic_header_rehydrates_nothing() {
        let mut rehydrated = [EnvSlot::empty(); MAX_ENVS];
        let (count, corrupt) = parse_envs(
            "env 0 1 1 00000000 00000000 0 0 0 1 2\n",
            &mut rehydrated,
            &mut policy_table(),
        );
        assert_eq!((count, corrupt), (0, 1));
        assert!(!rehydrated[0].occupied);
    }

    #[test]
    fn foreign_version_manifest_line_is_corrupt() {
        let text = "runtime-envs1\nenv 0 1 1 00000000 00000000 0 0 0 1 2\nmanifest 99 f -\n";
        let mut rehydrated = [EnvSlot::empty(); MAX_ENVS];
        let (count, corrupt) = parse_envs(text, &mut rehydrated, &mut policy_table());
        assert_eq!((count, corrupt), (1, 1));
        assert!(rehydrated[0].manifest.is_none());
    }

    #[test]
    fn duplicate_env_header_is_corrupt() {
        let text = "runtime-envs1\nenv 0 1 1 00000000 00000000 0 0 0 1 2\nenv 0 1 1 00000000 00000000 0 0 0 1 2\n";
        let mut rehydrated = [EnvSlot::empty(); MAX_ENVS];
        let (count, corrupt) = parse_envs(text, &mut rehydrated, &mut policy_table());
        assert_eq!((count, corrupt), (1, 1));
    }

    #[test]
    fn detail_lines_without_header_are_corrupt() {
        let text = "runtime-envs1\nmount 2f6461746120 2f6461746120\n";
        let mut rehydrated = [EnvSlot::empty(); MAX_ENVS];
        let (count, corrupt) = parse_envs(text, &mut rehydrated, &mut policy_table());
        assert_eq!((count, corrupt), (0, 1));
    }

    #[test]
    fn oversize_or_odd_hex_detail_lines_are_corrupt() {
        let long_guest = "2f".repeat(MAX_GUEST_PATH + 1);
        let text = format!(
            "runtime-envs1\nenv 0 1 1 00000000 00000000 0 0 0 1 2\nmount {long_guest} 2f64617461\n"
        );
        let mut rehydrated = [EnvSlot::empty(); MAX_ENVS];
        let (count, corrupt) = parse_envs(&text, &mut rehydrated, &mut policy_table());
        assert_eq!((count, corrupt), (1, 1));
        assert_eq!(rehydrated[0].mount_count, 0);
    }

    #[test]
    fn write_through_codec_survives_create_then_approve() {
        // Simulates the write-through contract end to end at the codec
        // level: allocate (create), approve sensitive caps (decision), then
        // persist; the rehydrated record carries the granted subset and the
        // Ready state, and the id (slot index) is unchanged.
        let mut envs = [EnvSlot::empty(); MAX_ENVS];
        let (env_id, pending, enforced) = allocate_env(
            &mut envs,
            rt::RuntimeKind::Posix as u32 as u64,
            sample_profile(),
            &policy_table(),
        )
        .ok()
        .unwrap();
        assert_eq!(env_id, 0);
        assert!(pending != 0);
        assert_eq!(enforced, None);
        let (state, granted) = apply_decision(
            envs[0].capabilities,
            envs[0].granted_caps,
            PermissionPolicyState::Allowed,
            None,
        );
        envs[env_id as usize].state = state;
        envs[env_id as usize].granted_caps = granted;
        envs[env_id as usize].sandbox.apply_granted_mask(granted);

        let mut buffer = [0u8; MAX_ENVSTORE_BYTES];
        let total = format_envs(&envs, &policy_table(), &mut buffer).expect("serialize");
        let text = core::str::from_utf8(&buffer[..total]).unwrap();
        let mut rehydrated = [EnvSlot::empty(); MAX_ENVS];
        let (count, corrupt) = parse_envs(text, &mut rehydrated, &mut policy_table());
        assert_eq!((count, corrupt), (1, 0));
        assert!(matches!(rehydrated[0].state, RuntimeEnvState::Ready));
        assert_eq!(rehydrated[0].granted_caps, granted);
        assert!(
            rehydrated[0]
                .sandbox
                .class_granted(crate::sandbox::DeviceClass::Network)
        );
    }

    fn policy_table() -> crate::policy::PolicyTable {
        crate::policy::PolicyTable::new()
    }

    #[test]
    fn codec_roundtrip_policy_section() {
        use crate::policy::EnvPolicyDefault;

        let mut policy = PolicyTable::new();
        policy.set(rt::RuntimeKind::Posix, EnvPolicyDefault::AllowAll);
        policy.set(rt::RuntimeKind::Windows, EnvPolicyDefault::DenyAll);

        let envs = [EnvSlot::empty(); MAX_ENVS];
        let mut buffer = [0u8; MAX_ENVSTORE_BYTES];
        let total = format_envs(&envs, &policy, &mut buffer).expect("serialize");
        let text = core::str::from_utf8(&buffer[..total]).unwrap();
        assert!(text.contains("policy 1 1\n"));
        assert!(text.contains("policy 2 2\n"));

        let mut rehydrated_policy = PolicyTable::new();
        let mut rehydrated = [EnvSlot::empty(); MAX_ENVS];
        let (count, corrupt) = parse_envs(text, &mut rehydrated, &mut rehydrated_policy);
        assert_eq!((count, corrupt), (0, 0));
        assert_eq!(rehydrated_policy, policy);
    }

    #[test]
    fn default_policy_serializes_byte_identically_to_pre_policy_grammar() {
        // All-Ask is the additive baseline: no policy lines, empty store
        // text stays exactly what the pre-policy build produced.
        let envs = [EnvSlot::empty(); MAX_ENVS];
        let mut buffer = [0u8; MAX_ENVSTORE_BYTES];
        let total = format_envs(&envs, &PolicyTable::new(), &mut buffer).expect("serialize");
        let text = core::str::from_utf8(&buffer[..total]).unwrap();
        assert_eq!(text, "runtime-envs1\n");
        assert!(!text.contains("policy "));
    }

    #[test]
    fn policy_lines_parse_before_any_env_header() {
        use crate::policy::EnvPolicyDefault;

        let text = "runtime-envs1\npolicy 1 1\nenv 0 1 1 00000000 00000000 0 0 0 1 2\nmount 2f64617461 2f64617461\n";
        let mut rehydrated = [EnvSlot::empty(); MAX_ENVS];
        let mut policy = PolicyTable::new();
        let (count, corrupt) = parse_envs(text, &mut rehydrated, &mut policy);
        assert_eq!((count, corrupt), (1, 0));
        assert!(matches!(
            policy.default_for(rt::RuntimeKind::Posix),
            EnvPolicyDefault::AllowAll
        ));
        // Detail lines still attach to the env header that follows them.
        assert_eq!(rehydrated[0].mount_count, 1);
    }

    #[test]
    fn malformed_policy_lines_are_corrupt() {
        for line in [
            "policy 1\n",
            "policy 1 2 3\n",
            "policy 3 1\n",
            "policy 1 7\n",
            "policy x 1\n",
            "policy 1 x\n",
            "policy 1 1\npolicy 1 1\n",
        ] {
            let text = format!("runtime-envs1\n{line}");
            let mut rehydrated = [EnvSlot::empty(); MAX_ENVS];
            let mut policy = PolicyTable::new();
            let (count, corrupt) = parse_envs(&text, &mut rehydrated, &mut policy);
            assert_eq!((count, corrupt), (0, 1), "line {line} must be corrupt");
        }
    }
}
