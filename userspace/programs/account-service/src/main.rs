//! Account service: owns the user account store and the login/identity
//! state machine on top of session-service session ids.
//!
//! Activation (manual, not in the default boot graph): the image is built
//! into the boot store as `services/account-service/program.img` and spawned
//! on demand via the manager's stored-image launch path
//! (`manager_launch_stored_program_with_payload` with that path). The service
//! is NOT registered under a named `ServiceId` — adding one would require a
//! shared-ABI change — so clients receive its public channel handle from the
//! launcher, not from bootstrap lookup.
//!
//! Startup handle convention (positional, all optional beyond zero):
//! handles[0] = storage-service channel. When present the account store is
//! loaded from `state/account/accounts.cfg` and re-written after every
//! persisted mutation (account creation, password changes, PBKDF2 upgrades
//! of legacy credential records); without it the store lives in memory
//! seeded with defaults.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

mod protocol;

use core::str;

use rt::{ControlTag, LifecycleEvent, RawMessage};
use serviceos_account_service::{ACCOUNTS_PATH, AccountStore, format_store, parse_store};

use crate::protocol::RequestScratch;

use serviceos_userspace_runtime as rt;

// Full 8-account store with PBKDF2 records: each PBKDF2 line adds ~190
// chars (algorithm marker, iteration count, 32-hex salt, 128-hex hash) on
// top of the ~135-char legacy prefix, so 2048 no longer bounds 8 records.
const MAX_STORE_BYTES: usize = 4096;
const ACCOUNTS_DIR: &str = "state/account";
const EXIT_OK: u64 = 0;
const EXIT_STARTUP: u64 = 0xfa01;
const EXIT_STORE: u64 = 0xfa02;
const EXIT_LOOP: u64 = 0xfa03;

#[cfg(not(test))]
rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return EXIT_STARTUP;
    }
    if startup.tag != ControlTag::Startup as u32 {
        return EXIT_STARTUP;
    }

    // Launch contract (positional): handles[0] = launcher handshake channel,
    // handles[1] = storage-service channel (manager launch grant for the
    // setup-wizard launch); single-handle launches keep the historical
    // handles[0] = storage convention. With fewer handles the store lives in
    // memory.
    let mut storage_handle = if startup.handle_count >= 2 {
        Some(startup.handles[1])
    } else if startup.handle_count == 1 {
        Some(startup.handles[0])
    } else {
        None
    };

    // Public control channel; handed to clients by whoever spawns us. For
    // launcher handshakes the send-half goes out over handles[0] (legacy
    // wizard launches); shell-driven launches publish nothing and rely on
    // per-launcher delivery.
    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return EXIT_STORE,
    };
    if startup.handle_count >= 2 {
        let mut announce = RawMessage::empty(0);
        announce.word_count = 1;
        announce.handle_count = 1;
        announce.handles[0] = public.second;
        announce.handle_rights[0] = rt::rights::SEND | rt::rights::DUPLICATE | rt::rights::TRANSFER;
        let _ = rt::channel_send(startup.handles[0], &announce);
    }

    let mut store = match load_store(storage_handle) {
        Ok(store) => store,
        // Unreadable or malformed persisted state: keep serving identities
        // from an in-memory default store instead of refusing to start.
        Err(_) => {
            storage_handle = None;
            AccountStore::seed_defaults()
        }
    };
    // Boot tick mixed into every fresh PBKDF2 salt. Honesty: this kernel has
    // no RNG yet, so salts are unique-ish boot-local substitutes, not
    // cryptographically random (see `pbkdf2_salt`).
    store.salt_tick = rt::monotonic_now().unwrap_or(0);

    loop {
        if lifecycle_stop_requested(bootstrap) {
            persist_store(storage_handle, &store);
            if let Some(handle) = storage_handle {
                let _ = rt::handle_close(handle);
            }
            return EXIT_OK;
        }

        let mut request = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut request) {
            Ok(()) => {
                let _ = rt::write_logf(
                    "account",
                    format_args!(
                        "request tag={:#x} words={} handles={}",
                        request.tag, request.word_count, request.handle_count
                    ),
                );
                let mut scratch = RequestScratch::new();
                let mut response = RawMessage::empty(0);
                let store_dirty =
                    protocol::handle_request(&mut store, &request, &mut response, &mut scratch);
                if store_dirty {
                    persist_store(storage_handle, &store);
                }
                if response.tag != 0 {
                    let sent = rt::channel_send(request.handles[0], &response);
                    if sent.is_err() {
                        let _ = rt::write_logf(
                            "account",
                            format_args!("reply send failed tag={:#x}", response.tag),
                        );
                    }
                    let _ = rt::handle_close(request.handles[0]);
                }
            }
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => return EXIT_LOOP,
        }

        if rt::yield_current().is_err() {
            return EXIT_LOOP;
        }
    }
}

fn lifecycle_stop_requested(bootstrap: rt::Handle) -> bool {
    let mut lifecycle = RawMessage::empty(0);
    match rt::channel_receive_nonblocking(bootstrap, &mut lifecycle) {
        Ok(()) => {
            lifecycle.tag == ControlTag::Lifecycle as u32
                && lifecycle.word_count >= 1
                && lifecycle.words[0] == LifecycleEvent::Stopped as u32 as u64
        }
        Err(rt::Error::QueueEmpty) => false,
        Err(_) => false,
    }
}

/// Bounded storage round-trip: send `request` and poll for the reply until
/// `timeout_ticks` elapse. The kernel's timed-receive flag is unreliable, so
/// bounded waits are built from nonblocking receives plus yields.
fn storage_rpc(
    endpoint: rt::Handle,
    request: &mut RawMessage,
    timeout_ticks: u64,
) -> Result<RawMessage, ()> {
    let pair = rt::channel_create().map_err(|_| ())?;
    request.handle_count = 1;
    request.handles[0] = pair.second;
    request.handle_rights[0] = rt::rights::SEND;
    let send_result = rt::channel_send(endpoint, request);
    let _ = rt::handle_close(pair.second);
    send_result.map_err(|_| ())?;

    let deadline = rt::monotonic_now()
        .unwrap_or(0)
        .saturating_add(timeout_ticks);
    let response = loop {
        let mut received = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(pair.first, &mut received) {
            Ok(()) => break received,
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => {
                let _ = rt::handle_close(pair.first);
                return Err(());
            }
        }
        if rt::monotonic_now().unwrap_or(0) >= deadline {
            let _ = rt::handle_close(pair.first);
            return Err(());
        }
        let _ = rt::yield_current();
    };
    let _ = rt::handle_close(pair.first);
    Ok(response)
}

fn load_store(storage_handle: Option<rt::Handle>) -> Result<AccountStore, ()> {
    const OPEN_REQUEST: u32 = 0x500;
    const OPEN_REPLY: u32 = 0x501;
    const OPEN_OK: u32 = 0;
    const OPEN_NOT_FOUND: u32 = 1;

    let Some(storage_handle) = storage_handle else {
        return Ok(AccountStore::seed_defaults());
    };
    let mut bytes = [0u8; MAX_STORE_BYTES];
    let mut request = RawMessage::empty(OPEN_REQUEST);
    request.word_count = 1 + ((ACCOUNTS_PATH.len() + 7) / 8) as u32;
    request.words[0] = ACCOUNTS_PATH.len() as u64;
    let mut cursor = 1usize;
    for group in ACCOUNTS_PATH.as_bytes().chunks(8) {
        let mut packed = [0u8; 8];
        packed[..group.len()].copy_from_slice(group);
        request.words[cursor] = u64::from_le_bytes(packed);
        cursor += 1;
    }
    let response = storage_rpc(storage_handle, &mut request, 300)?;
    if response.tag != OPEN_REPLY || response.word_count < 2 {
        return Err(());
    }
    match response.words[0] as u32 {
        OPEN_NOT_FOUND => 0,
        OPEN_OK => {
            let blob = response.handles[0];
            let len = (response.words[1] as usize).min(bytes.len());
            let mut offset = 0usize;
            while offset < len {
                match rt::storage_read(blob, offset, &mut bytes[offset..len]) {
                    Ok(0) => break,
                    Ok(read) => offset += read,
                    Err(_) => {
                        let _ = rt::storage_blob_close(blob);
                        return Err(());
                    }
                }
            }
            let _ = rt::storage_blob_close(blob);
            len
        }
        _ => return Err(()),
    };
    if bytes.iter().all(|&byte| byte == 0) {
        // First boot: seed defaults so the next mutation persists them.
        return Ok(AccountStore::seed_defaults());
    }
    let text = str::from_utf8(&bytes).map_err(|_| ())?;
    let text = text.trim_end_matches('\0');
    parse_store(text).map_err(|_| ())
}

fn persist_store(storage_handle: Option<rt::Handle>, store: &AccountStore) {
    let Some(storage_handle) = storage_handle else {
        return;
    };
    let dir = match ensure_account_dir(storage_handle) {
        Ok(dir) => dir,
        Err(_) => return,
    };
    let file = match rt::storage_directory_open_file(dir, "accounts.cfg", true, true) {
        Ok((file, _)) => file,
        Err(_) => {
            let _ = rt::handle_close(dir);
            return;
        }
    };
    let mut buffer = [0u8; MAX_STORE_BYTES];
    let total = format_store(store, &mut buffer).unwrap_or(0);
    let mut offset = 0usize;
    while offset < total {
        let chunk_len = (total - offset).min((rt::IPC_MAX_WORDS - 3) * 8);
        if rt::storage_write(file, offset, total, &buffer[offset..offset + chunk_len]).is_err() {
            break;
        }
        offset += chunk_len;
    }
    let _ = rt::storage_blob_close(file);
    let _ = rt::handle_close(dir);
}

fn ensure_account_dir(storage_handle: rt::Handle) -> Result<rt::Handle, ()> {
    if let Ok(dir) = rt::storage_open_directory(storage_handle, ACCOUNTS_DIR, true) {
        return Ok(dir);
    }
    let state = rt::storage_open_directory(storage_handle, "state", true).map_err(|_| ())?;
    let created = rt::storage_directory_create(state, "account", rt::StorageEntryKind::Directory);
    let _ = rt::handle_close(state);
    created.map_err(|_| ())?;
    rt::storage_open_directory(storage_handle, ACCOUNTS_DIR, true).map_err(|_| ())
}
