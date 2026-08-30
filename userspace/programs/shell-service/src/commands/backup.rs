//! Operator backup surface: `backup` subcommands drive the manually-activated
//! backup-service (snapshot listing, export, restore with dry-run report,
//! deletion). Reachability mirrors the peripheral-command precedent: the
//! image lives in the boot store as `services/backup-service/program.img`
//! and is launched on demand through the manager's stored-image path, whose
//! launch handshake (storage grant first, announcer second) carries the
//! service's published public channel back to this shell. Protocol tags are
//! the ones published by `serviceos_backup_service::backup_tag` (0x230-0x237,
//! status-first replies, 0 = Ok). When launch fails, backup surfaces degrade
//! to notices — activation is manual, so absence is a normal state.
//!
//! Snapshot names stay plain `backup-<tick>` tokens end to end: the shell
//! validates them with `parse_backup_name` and never passes raw paths, while
//! the service path-confines everything under `backups/` on its side.

use core::cell::UnsafeCell;

use rt::{Handle, RawMessage};
use serviceos_backup_service::{MAX_BACKUP_NAME, backup_tag, parse_backup_name, scope};
use serviceos_userspace_runtime as rt;

use crate::util::{ShellOutput, write_output_linef};

/// Boot-store location of the backup-service image (manual activation).
pub const BACKUP_PROGRAM_PATH: &str = "services/backup-service/program.img";

/// Capacity-bounded snapshot listing (service pages via index echo).
const LIST_MAX_ROWS: usize = 16;
const NAME_BUFFER: usize = 48;

struct BackupChannel {
    handle: Handle,
    reachable: bool,
}

struct CacheSlot(UnsafeCell<BackupChannel>);

// SAFETY: the shell task is strictly single-threaded; see the account-cache
// precedent in commands/account.rs.
unsafe impl Sync for CacheSlot {}

static BACKUP_CACHE: CacheSlot = CacheSlot(UnsafeCell::new(BackupChannel {
    handle: rt::INVALID_HANDLE,
    reachable: false,
}));

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackupFlow {
    /// Stored-image launch failed or was denied.
    Unavailable,
    /// Service replied with a non-zero status code.
    Rejected(u64),
    /// Wire-level failure talking to the service.
    Transport,
}

impl BackupFlow {
    pub const fn message(self) -> &'static str {
        match self {
            BackupFlow::Unavailable => {
                "backup-service unavailable (not in boot store or launch denied)"
            }
            BackupFlow::Rejected(1) => "backup-service rejected: invalid argument",
            BackupFlow::Rejected(2) => "backup-service rejected: unknown scope",
            BackupFlow::Rejected(3) => "backup-service rejected: capacity exceeded",
            BackupFlow::Rejected(4) => "backup-service rejected: snapshot not found",
            BackupFlow::Rejected(5) | BackupFlow::Rejected(6) | BackupFlow::Rejected(7) => {
                "backup-service rejected: snapshot failed checksum or format validation"
            }
            BackupFlow::Rejected(8) => "backup-service rejected: storage failure",
            BackupFlow::Rejected(_) => "backup-service rejected the request",
            BackupFlow::Transport => "backup-service transport failure",
        }
    }
}

fn cache() -> &'static mut BackupChannel {
    // SAFETY: single-threaded shell task.
    unsafe { &mut *BACKUP_CACHE.0.get() }
}

fn ensure_backup_channel(bootstrap: rt::Handle) -> Option<Handle> {
    let slot = cache();
    if slot.reachable && slot.handle != rt::INVALID_HANDLE {
        return Some(slot.handle);
    }
    // Storage grant first (the service exits without it), announcer second:
    // the service publishes its public channel over the announcer handshake.
    let handle = super::account::launch_with_announce(bootstrap, BACKUP_PROGRAM_PATH, true)?;
    slot.handle = handle;
    slot.reachable = true;
    Some(handle)
}

fn call(bootstrap: rt::Handle, tag: u32, words: &[u64]) -> Result<RawMessage, BackupFlow> {
    let Some(handle) = ensure_backup_channel(bootstrap) else {
        return Err(BackupFlow::Unavailable);
    };
    let mut request = RawMessage::empty(tag);
    request.word_count = words.len() as u32;
    request.words[..words.len()].copy_from_slice(words);
    let response = rt::channel_call(handle, &mut request).map_err(|_| BackupFlow::Transport)?;
    if response.word_count < 1 {
        return Err(BackupFlow::Transport);
    }
    Ok(response)
}

fn status_reply(flow: Result<RawMessage, BackupFlow>) -> Result<u64, BackupFlow> {
    let reply = flow?;
    Ok(reply.words[0])
}

/// Scope-name lookup shared with command parsing
/// (`export [scope]`, scope = config|accounts|packages|all).
pub fn parse_scope_name(name: Option<&str>) -> Option<u32> {
    match name {
        None | Some("all") => Some(scope::KNOWN_MASK),
        Some("config") => Some(scope::CONFIG),
        Some("accounts") => Some(scope::ACCOUNTS),
        Some("packages") => Some(scope::PACKAGES),
        Some(_) => None,
    }
}

/// Parse `<name> [--yes]`: the target first, then at most one --yes flag.
/// Any other token is a usage error; a leading --yes means the name itself
/// is missing.
fn parse_target_args<'a>(
    args: &mut core::str::SplitWhitespace<'a>,
) -> Result<(Option<&'a str>, bool), ()> {
    let name = match args.next() {
        Some("--yes") | None => return Err(()),
        Some(name) => Some(name),
    };
    let mut yes = false;
    for token in args {
        if token == "--yes" && !yes {
            yes = true;
        } else {
            return Err(());
        }
    }
    Ok((name, yes))
}

/// A plain `backup-<tick>` snapshot name; anything else (raw paths, empty
/// strings) is refused before it ever reaches the service.
fn validate_snapshot_name(name: &str) -> bool {
    parse_backup_name(name).is_some()
}

/// Unpack a packed name field (`[len, packed...]` starting at `offset`)
/// into a printable &str backed by `buffer`.
fn unpack_name<'a>(reply: &'a RawMessage, offset: usize, buffer: &'a mut [u8]) -> Option<&'a str> {
    let len = *reply.words.get(offset)? as usize;
    if len == 0 || len > buffer.len() {
        return None;
    }
    rt::unpack_bytes(
        &reply.words[offset + 1..reply.word_count as usize],
        len,
        buffer,
    )
    .ok()?;
    core::str::from_utf8(&buffer[..len]).ok()
}

pub(crate) fn cmd_backup(
    bootstrap: rt::Handle,
    output: ShellOutput,
    parts: core::str::SplitWhitespace<'_>,
) -> rt::Result<()> {
    let mut parts = parts;
    match parts.next() {
        None | Some("list") => cmd_list(bootstrap, output),
        Some("export") => cmd_export(bootstrap, output, parts.next()),
        Some("restore") => cmd_restore(bootstrap, output, parts),
        Some("delete") => cmd_delete(bootstrap, output, parts),
        Some(_) => write_output_linef(
            output,
            format_args!(
                "usage: backup [list|export [scope]|restore <name> [--yes]|delete <name> [--yes]]"
            ),
        ),
    }
}

fn cmd_list(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let mut rows = [SnapshotRow::EMPTY; LIST_MAX_ROWS];
    let mut count = 0usize;
    for index in 0..LIST_MAX_ROWS {
        match call(bootstrap, backup_tag::LIST_REQUEST, &[index as u64]) {
            Ok(reply) if reply.words[0] == 0 => {
                let mut buffer = [0u8; NAME_BUFFER];
                let Some(name) = unpack_name(&reply, 3, &mut buffer) else {
                    return write_output_linef(
                        output,
                        format_args!("{}", BackupFlow::Transport.message()),
                    );
                };
                if count < rows.len() {
                    rows[count].set(name);
                }
                count += 1;
            }
            Ok(reply) if reply.words[0] == 2 => break,
            Ok(reply) => {
                return write_output_linef(
                    output,
                    format_args!("{}", BackupFlow::Rejected(reply.words[0]).message()),
                );
            }
            Err(flow) => {
                return write_output_linef(output, format_args!("{}", flow.message()));
            }
        }
    }
    if count == 0 {
        return write_output_linef(output, format_args!("snapshots: none"));
    }
    let shown = count.min(rows.len());
    write_output_linef(output, format_args!("snapshots: {}", shown))?;
    for row in rows.iter().take(shown) {
        write_output_linef(output, format_args!("{}", row.text().unwrap_or("?")))?;
    }
    Ok(())
}

/// Fixed-capacity snapshot row: bounded name bytes plus length, so the
/// listing needs no heap (the shell runs no_std).
struct SnapshotRow {
    bytes: [u8; NAME_BUFFER],
    len: usize,
}

impl SnapshotRow {
    const EMPTY: Self = Self {
        bytes: [0; NAME_BUFFER],
        len: 0,
    };

    fn set(&mut self, name: &str) {
        let bytes = name.as_bytes();
        let len = bytes.len().min(NAME_BUFFER);
        self.bytes[..len].copy_from_slice(&bytes[..len]);
        self.len = len;
    }

    fn text(&self) -> Option<&str> {
        core::str::from_utf8(&self.bytes[..self.len]).ok()
    }
}

fn cmd_export(
    bootstrap: rt::Handle,
    output: ShellOutput,
    scope_arg: Option<&str>,
) -> rt::Result<()> {
    let Some(mask) = parse_scope_name(scope_arg) else {
        return write_output_linef(
            output,
            format_args!("unknown scope; try config|accounts|packages|all"),
        );
    };
    let flow = call(bootstrap, backup_tag::EXPORT_REQUEST, &[mask as u64]);
    match flow {
        Ok(reply) if reply.word_count >= 4 && reply.words[0] == 0 => {
            let mut buffer = [0u8; NAME_BUFFER];
            let name = unpack_name(&reply, 1, &mut buffer).unwrap_or("?");
            write_output_linef(
                output,
                format_args!(
                    "exported {}: records={} bytes={}",
                    name, reply.words[2], reply.words[3]
                ),
            )
        }
        Ok(reply) => write_output_linef(
            output,
            format_args!(
                "{}",
                BackupFlow::Rejected(reply.words.first().copied().unwrap_or(0)).message()
            ),
        ),
        Err(flow) => write_output_linef(output, format_args!("{}", flow.message())),
    }
}

fn cmd_restore(
    bootstrap: rt::Handle,
    output: ShellOutput,
    mut args: core::str::SplitWhitespace<'_>,
) -> rt::Result<()> {
    let Ok((name_arg, yes)) = parse_target_args(&mut args) else {
        return write_output_linef(output, format_args!("usage: backup restore <name> [--yes]"));
    };
    let Some(name) = name_arg else {
        return write_output_linef(output, format_args!("usage: backup restore <name> [--yes]"));
    };
    if !validate_snapshot_name(name) {
        return write_output_linef(
            output,
            format_args!("invalid snapshot name (expected backup-<id>)"),
        );
    }

    // Dry-run report first, always: the service validates the blob
    // (magic/version/checksum) without writing anything.
    let mut request_words = [0u64; 8];
    request_words[0] = scope::KNOWN_MASK as u64;
    request_words[1] = 1;
    let packed = pack_name(name, &mut request_words[2..]);
    let Some(packed) = packed else {
        return write_output_linef(output, format_args!("invalid snapshot name"));
    };
    let flow = call(
        bootstrap,
        backup_tag::RESTORE_REQUEST,
        &request_words[..2 + packed],
    );
    let reply = match flow {
        Ok(reply) if reply.word_count >= 5 && reply.words[0] == 0 => reply,
        Ok(reply) => {
            return write_output_linef(
                output,
                format_args!(
                    "{}",
                    BackupFlow::Rejected(reply.words.first().copied().unwrap_or(0)).message()
                ),
            );
        }
        Err(flow) => {
            return write_output_linef(output, format_args!("{}", flow.message()));
        }
    };
    write_output_linef(
        output,
        format_args!(
            "would restore records={} bytes={} scopes={:#x}",
            reply.words[3], reply.words[4], reply.words[2]
        ),
    )?;
    if !yes {
        return write_output_linef(output, format_args!("dry run only; add --yes to apply"));
    }

    request_words[1] = 0;
    let flow = call(
        bootstrap,
        backup_tag::RESTORE_REQUEST,
        &request_words[..2 + packed],
    );
    match flow {
        Ok(reply) if reply.word_count >= 5 && reply.words[0] == 0 => write_output_linef(
            output,
            format_args!(
                "restored records={} bytes={} scopes={:#x}",
                reply.words[3], reply.words[4], reply.words[2]
            ),
        ),
        Ok(reply) => write_output_linef(
            output,
            format_args!(
                "{}",
                BackupFlow::Rejected(reply.words.first().copied().unwrap_or(0)).message()
            ),
        ),
        Err(flow) => write_output_linef(output, format_args!("{}", flow.message())),
    }
}

fn cmd_delete(
    bootstrap: rt::Handle,
    output: ShellOutput,
    mut args: core::str::SplitWhitespace<'_>,
) -> rt::Result<()> {
    let Ok((name_arg, yes)) = parse_target_args(&mut args) else {
        return write_output_linef(output, format_args!("usage: backup delete <name> [--yes]"));
    };
    let Some(name) = name_arg else {
        return write_output_linef(output, format_args!("usage: backup delete <name> [--yes]"));
    };
    if !validate_snapshot_name(name) {
        return write_output_linef(
            output,
            format_args!("invalid snapshot name (expected backup-<id>)"),
        );
    }
    if !yes {
        return write_output_linef(
            output,
            format_args!("refusing to delete {} without --yes (destructive)", name),
        );
    }
    let mut request_words = [0u64; 8];
    let Some(packed) = pack_name(name, &mut request_words[1..]) else {
        return write_output_linef(output, format_args!("invalid snapshot name"));
    };
    request_words[0] = name.len() as u64;
    match status_reply(call(
        bootstrap,
        backup_tag::DELETE_REQUEST,
        &request_words[..1 + packed],
    )) {
        Ok(0) => write_output_linef(output, format_args!("deleted {}", name)),
        Ok(code) => write_output_linef(
            output,
            format_args!("{}", BackupFlow::Rejected(code).message()),
        ),
        Err(flow) => write_output_linef(output, format_args!("{}", flow.message())),
    }
}

/// Pack a length-prefixed name into `words`; returns words consumed.
fn pack_name(name: &str, words: &mut [u64]) -> Option<usize> {
    let bytes = name.as_bytes();
    if bytes.len() > MAX_BACKUP_NAME {
        return None;
    }
    words[0] = bytes.len() as u64;
    let packed = rt::pack_bytes(bytes, &mut words[1..]).ok()? as usize;
    Some(packed + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_names_map_onto_the_service_mask() {
        assert_eq!(parse_scope_name(None), Some(scope::KNOWN_MASK));
        assert_eq!(parse_scope_name(Some("all")), Some(scope::KNOWN_MASK));
        assert_eq!(parse_scope_name(Some("config")), Some(scope::CONFIG));
        assert_eq!(parse_scope_name(Some("accounts")), Some(scope::ACCOUNTS));
        assert_eq!(parse_scope_name(Some("packages")), Some(scope::PACKAGES));
        assert_eq!(parse_scope_name(Some("toaster")), None);
        assert_eq!(parse_scope_name(Some("")), None);
    }

    #[test]
    fn target_args_accept_name_and_single_yes_flag_only() {
        let mut args = "backup-7 --yes".split_whitespace();
        assert_eq!(parse_target_args(&mut args), Ok((Some("backup-7"), true)));
        let mut args = "backup-7".split_whitespace();
        assert_eq!(parse_target_args(&mut args), Ok((Some("backup-7"), false)));
        let mut args = "".split_whitespace();
        assert_eq!(parse_target_args(&mut args), Err(()));
        let mut args = "--yes".split_whitespace();
        assert_eq!(parse_target_args(&mut args), Err(()));
        let mut args = "backup-7 --yes --yes".split_whitespace();
        assert_eq!(parse_target_args(&mut args), Err(()));
        let mut args = "backup-7 --force".split_whitespace();
        assert_eq!(parse_target_args(&mut args), Err(()));
    }

    #[test]
    fn snapshot_names_stay_plain_tokens_never_paths() {
        assert!(validate_snapshot_name("backup-1724600000123"));
        assert!(validate_snapshot_name("backup-1"));
        assert!(!validate_snapshot_name("backups/backup-1"));
        assert!(!validate_snapshot_name("backup-"));
        assert!(!validate_snapshot_name(""));
        assert!(!validate_snapshot_name("../state/account/accounts.cfg"));
        assert!(!validate_snapshot_name("backup-12ab"));
    }

    #[test]
    fn flow_messages_stay_operator_readable() {
        assert_eq!(
            BackupFlow::Unavailable.message(),
            "backup-service unavailable (not in boot store or launch denied)"
        );
        assert_eq!(
            BackupFlow::Rejected(4).message(),
            "backup-service rejected: snapshot not found"
        );
        assert_eq!(
            BackupFlow::Rejected(7).message(),
            "backup-service rejected: snapshot failed checksum or format validation"
        );
        assert!(BackupFlow::Transport.message().starts_with("backup"));
    }

    #[test]
    fn pack_name_length_prefixes_snapshot_tokens() {
        let mut words = [0u64; 8];
        let packed = pack_name("backup-42", &mut words).expect("fits");
        assert_eq!(words[0], 9);
        assert_eq!(packed, 1 + 2);
        assert_eq!(
            pack_name(&"x".repeat(MAX_BACKUP_NAME + 1), &mut words),
            None
        );
    }
}
