//! Session ownership policy: bind operator sessions to an account-service
//! identity when the service is reachable.
//!
//! Reachability follows account-service's own activation model: the image
//! lives in the boot store as `services/account-service/program.img` and is
//! launched on demand through the manager's stored-image path, whose reply
//! carries the service's public channel handle. The protocol tags are the
//! ones published by `serviceos_account_service::account_tag`. When launch or
//! login fails, sessions simply stay unowned — activation is manual, so
//! unowned operation is normal and every other shell feature keeps working.

use core::cell::UnsafeCell;

use rt::{Handle, RawMessage};
use serviceos_account_service::{MAX_NAME, account_tag};
use serviceos_userspace_runtime as rt;

/// Secret length cap mirrors the account-service protocol scratch field
/// (MAX_SECRET is private to that crate's protocol module).
const MAX_SECRET: usize = 64;

/// Boot-store location of the account-service image (manual activation).
pub const ACCOUNT_PROGRAM_PATH: &str = "services/account-service/program.img";

struct AccountChannel {
    handle: Handle,
    reachable: bool,
}

struct CacheSlot(UnsafeCell<AccountChannel>);

// SAFETY: the shell task is strictly single-threaded; see the pending-line
// precedent in util/pending.rs.
unsafe impl Sync for CacheSlot {}

static ACCOUNT_CACHE: CacheSlot = CacheSlot(UnsafeCell::new(AccountChannel {
    handle: rt::INVALID_HANDLE,
    reachable: false,
}));

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountFlow {
    /// Stored-image launch failed or was denied: no ownership enforcement.
    Unavailable,
    /// Account service replied with a non-zero status code.
    Rejected(u64),
    /// Wire-level failure talking to the service.
    Transport,
}

impl AccountFlow {
    pub const fn message(self) -> &'static str {
        match self {
            AccountFlow::Unavailable => {
                "account-service unavailable (not in boot store or launch denied); \
                 session stays unowned"
            }
            AccountFlow::Rejected(1) => "login rejected: invalid argument",
            AccountFlow::Rejected(3) => "login rejected: duplicate name",
            AccountFlow::Rejected(4) => "login rejected: unknown account",
            AccountFlow::Rejected(5) => "login rejected: bad credentials",
            AccountFlow::Rejected(_) => "login rejected by account-service",
            AccountFlow::Transport => "account-service transport failure",
        }
    }
}

fn cache() -> &'static mut AccountChannel {
    // SAFETY: single-threaded shell task.
    unsafe { &mut *ACCOUNT_CACHE.0.get() }
}

/// Fetch (launching on demand) the account-service public channel, caching
/// successes. Failures are not cached so later logins can succeed once the
/// image exists.
///
/// Handle order matters: account-service's positional startup contract is
/// handles[0] = launcher handshake (the announce target), handles[1] =
/// storage. The shared launch_with_announce orders storage first (the
/// backup-service convention), so account-service gets its own sequence.
fn ensure_account_channel(bootstrap: rt::Handle) -> Option<Handle> {
    let slot = cache();
    if slot.reachable && slot.handle != rt::INVALID_HANDLE {
        return Some(slot.handle);
    }
    let handle = launch_account_service(bootstrap)?;
    slot.handle = handle;
    slot.reachable = true;
    Some(handle)
}

/// Launch account-service with its own positional contract: the announcer
/// carry must sit at handles[0] (account-service announces its public
/// send-half there) and the storage grant at handles[1].
fn launch_account_service(bootstrap: rt::Handle) -> Option<Handle> {
    const ANNOUNCE_WAIT_ITERATIONS: usize = 5000;

    let storage = match rt::lookup_service(bootstrap, rt::ServiceId::Storage) {
        Ok(handle) => handle,
        Err(_) => return None,
    };
    let announcer = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => {
            let _ = rt::handle_close(storage);
            return None;
        }
    };
    let startup_handles: [rt::StartupHandle; 2] = [
        rt::StartupHandle {
            handle: announcer.second,
            // Relay hops (shell -> manager -> child) need a re-forwardable
            // copy; a send-only mask dies at the first hop.
            rights: rt::rights::SEND | rt::rights::DUPLICATE | rt::rights::TRANSFER,
        },
        rt::StartupHandle {
            handle: storage,
            rights: rt::rights::SEND | rt::rights::DUPLICATE | rt::rights::TRANSFER,
        },
    ];
    let launched = rt::manager_launch_stored_program_with_payload(
        bootstrap,
        ACCOUNT_PROGRAM_PATH,
        &[],
        &startup_handles,
    );
    let _ = rt::handle_close(announcer.second);
    let _ = rt::handle_close(storage);
    if launched.is_err() {
        let _ = rt::handle_close(announcer.first);
        return None;
    }

    // Await the child's announce carrying its public send-half.
    for _ in 0..ANNOUNCE_WAIT_ITERATIONS {
        let mut message = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(announcer.first, &mut message) {
            Ok(()) => {
                let _ = rt::handle_close(announcer.first);
                if message.handle_count >= 1 {
                    return Some(message.handles[0]);
                }
                return None;
            }
            Err(rt::Error::QueueEmpty) => {
                if rt::yield_current().is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let _ = rt::handle_close(announcer.first);
    None
}

/// Launch a stored program and await its public-channel announcement.
///
/// The manager's launch reply carries a task view, not a channel, so the
/// caller passes an announcer send-half as a startup handle; the launched
/// service replies with its public channel's send-half. `with_storage`
/// prepends the storage-service grant first (account-service's positional
/// contract: handles[0]=storage, handles[1]=announcer); services that only
/// need the announcer keep it at handles[0].
pub(crate) fn launch_with_announce(
    bootstrap: rt::Handle,
    program_path: &str,
    with_storage: bool,
) -> Option<Handle> {
    const ANNOUNCE_WAIT_ITERATIONS: usize = 5000;

    let mut startup_handles: [rt::StartupHandle; 2] = [
        rt::StartupHandle {
            handle: rt::INVALID_HANDLE,
            rights: 0,
        },
        rt::StartupHandle {
            handle: rt::INVALID_HANDLE,
            rights: 0,
        },
    ];
    let mut count = 0usize;
    if with_storage {
        let storage = rt::lookup_service(bootstrap, rt::ServiceId::Storage).ok()?;
        startup_handles[count] = rt::StartupHandle {
            handle: storage,
            // The relay hops (shell -> manager -> child) each need a
            // re-forwardable copy; a send-only mask dies at the first hop
            // (see root-manager lookup.rs's rights note).
            rights: rt::rights::SEND | rt::rights::DUPLICATE | rt::rights::TRANSFER,
        };
        count += 1;
    }
    let announcer = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return None,
    };
    startup_handles[count] = rt::StartupHandle {
        handle: announcer.second,
        rights: rt::rights::SEND | rt::rights::DUPLICATE | rt::rights::TRANSFER,
    };
    count += 1;

    let launched = rt::manager_launch_stored_program_with_payload(
        bootstrap,
        program_path,
        &[],
        &startup_handles[..count],
    );
    let _ = rt::handle_close(announcer.second);
    if launched.is_err() {
        let _ = rt::handle_close(announcer.first);
        return None;
    }

    // Await the child's announce carrying its public send-half.
    for _ in 0..ANNOUNCE_WAIT_ITERATIONS {
        let mut message = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(announcer.first, &mut message) {
            Ok(()) => {
                let _ = rt::handle_close(announcer.first);
                if message.handle_count >= 1 {
                    return Some(message.handles[0]);
                }
                return None;
            }
            Err(rt::Error::QueueEmpty) => {
                if rt::yield_current().is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let _ = rt::handle_close(announcer.first);
    None
}

/// Run one request/reply round against account-service.
fn account_call(
    account_handle: Handle,
    mut request: RawMessage,
) -> Result<RawMessage, AccountFlow> {
    let response =
        rt::channel_call(account_handle, &mut request).map_err(|_| AccountFlow::Transport)?;
    if response.word_count < 1 {
        return Err(AccountFlow::Transport);
    }
    Ok(response)
}

/// LOGIN_REQUEST: [name_len][name][secret_len][secret][session_id].
/// Reply: [status][account_id][session_id][capabilities].
pub fn login(
    bootstrap: rt::Handle,
    name: &str,
    secret: &str,
    session_id: u32,
) -> Result<(u32, u64), AccountFlow> {
    let Some(account_handle) = ensure_account_channel(bootstrap) else {
        return Err(AccountFlow::Unavailable);
    };
    let name_bytes = name.as_bytes();
    let secret_bytes = secret.as_bytes();
    if name_bytes.len() > MAX_NAME || secret_bytes.len() > MAX_SECRET {
        return Err(AccountFlow::Rejected(1));
    }

    let mut request = RawMessage::empty(account_tag::LOGIN_REQUEST);
    // Account-service decodes LOGIN name_len at words[0] (protocol.rs
    // decode_str(request, 0, ..)) — pack the fields from the first word.
    let mut word_count = 0usize;
    word_count += pack_field(name_bytes, &mut request.words[word_count..])?;
    word_count += pack_field(secret_bytes, &mut request.words[word_count..])?;
    *request
        .words
        .get_mut(word_count)
        .ok_or(AccountFlow::Transport)? = session_id as u64;
    word_count += 1;
    request.word_count = word_count as u32;

    let response = account_call(account_handle, request)?;
    let status = response.words[0];
    if status != 0 {
        return Err(AccountFlow::Rejected(status));
    }
    let account_id = *response.words.get(1).ok_or(AccountFlow::Transport)? as u32;
    let capabilities = *response.words.get(3).unwrap_or(&0);
    Ok((account_id, capabilities))
}

/// VERIFY_PASSWORD_REQUEST for the sshd bridge: [name_len][name]
/// [secret_len][secret]. Read-only credential check — no claim is created
/// and no record is upgraded (that stays on the interactive login path).
/// Returns Ok(true/false) from account-service; transport loss is
/// AccountFlow::Unavailable.
pub fn verify_password(
    bootstrap: rt::Handle,
    name: &str,
    secret: &str,
) -> Result<bool, AccountFlow> {
    let Some(account_handle) = ensure_account_channel(bootstrap) else {
        return Err(AccountFlow::Unavailable);
    };
    let name_bytes = name.as_bytes();
    let secret_bytes = secret.as_bytes();
    if name_bytes.len() > MAX_NAME || secret_bytes.len() > MAX_SECRET {
        return Ok(false);
    }

    let mut request = RawMessage::empty(account_tag::VERIFY_PASSWORD_REQUEST);
    // Same layout as LOGIN minus the session word: name_len at words[0].
    let mut word_count = 0usize;
    word_count += pack_field(name_bytes, &mut request.words[word_count..])?;
    word_count += pack_field(secret_bytes, &mut request.words[word_count..])?;
    request.word_count = word_count as u32;

    let response = account_call(account_handle, request)?;
    if response.word_count < 2 || response.words[0] != 0 {
        return Err(AccountFlow::Transport);
    }
    Ok(response.words[1] == 1)
}

/// LOGOUT_REQUEST for the bound operator-session id. Best effort: callers
/// clear local ownership regardless of the reply.
pub fn logout(bootstrap: rt::Handle, session_id: u32) -> Result<(), AccountFlow> {
    let Some(account_handle) = ensure_account_channel(bootstrap) else {
        return Err(AccountFlow::Unavailable);
    };
    let mut request = RawMessage::empty(account_tag::LOGOUT_REQUEST);
    request.words[0] = session_id as u64;
    request.word_count = 1;
    let response = account_call(account_handle, request)?;
    if response.words[0] != 0 {
        // Unknown/not-logged-in codes still end local ownership.
        return Ok(());
    }
    Ok(())
}

/// Pack one length-prefixed byte field starting at `words[0]`; returns the
/// number of words consumed (length word + packed payload words).
fn pack_field(bytes: &[u8], words: &mut [u64]) -> Result<usize, AccountFlow> {
    let required = bytes.len().div_ceil(8) + 1;
    if required > words.len() {
        return Err(AccountFlow::Rejected(1));
    }
    words[0] = bytes.len() as u64;
    // Account-service's string codec is BIG-endian per word (its own
    // pack_bytes/unpack_bytes pair); rt::pack_bytes is little-endian and
    // would scramble every byte field on the account wire.
    let packed = serviceos_account_service::pack_bytes(&mut words[1..], bytes);
    Ok(packed + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_packing_matches_account_service_decoder_layout() {
        let mut words = [0u64; 8];
        assert_eq!(pack_field(b"paul", &mut words).unwrap(), 2);
        assert_eq!(words[0], 4);
        // 'paul' big-endian in the next word (account-service's string
        // codec packs bytes from the top of each word).
        assert_eq!(words[1], 0x7061_756c_0000_0000);

        let mut tight = [0u64; 2];
        assert_eq!(
            pack_field(&[0u8; 16], &mut tight),
            Err(AccountFlow::Rejected(1))
        );
    }

    #[test]
    fn flow_messages_stay_operator_readable() {
        assert_eq!(
            AccountFlow::Unavailable.message(),
            "account-service unavailable (not in boot store or launch denied); session stays unowned"
        );
        assert_eq!(
            AccountFlow::Rejected(5).message(),
            "login rejected: bad credentials"
        );
        assert!(
            AccountFlow::Transport
                .message()
                .starts_with("account-service")
        );
    }

    #[test]
    fn oversize_credentials_are_rejected_before_the_wire() {
        // login() refuses names past MAX_NAME before packing; pack_field
        // itself enforces only the word budget of the target message.
        let long_name = "x".repeat(MAX_NAME + 1);
        let mut words = [0u64; 16];
        assert_eq!(pack_field(long_name.as_bytes(), &mut words).unwrap(), 6);

        // A full-size secret field still fits a standard request.
        let secret = [0u8; MAX_SECRET];
        assert_eq!(pack_field(&secret, &mut words).unwrap(), 9);
        // ...but not when the caller's word budget is too tight.
        let mut tight = [0u64; 2];
        assert_eq!(
            pack_field(&secret[..9], &mut tight),
            Err(AccountFlow::Rejected(1))
        );
    }
}
