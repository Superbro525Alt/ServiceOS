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
//! mutation; without it the store lives in memory seeded with defaults.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

mod protocol;

use core::str;

use rt::{ControlTag, LifecycleEvent, RawMessage};
use serviceos_account_service::{ACCOUNTS_PATH, AccountStore, format_store, parse_store};

use crate::protocol::RequestScratch;

use serviceos_userspace_runtime as rt;

const MAX_STORE_BYTES: usize = 2048;
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

    let storage_handle = if startup.handle_count >= 1 {
        Some(startup.handles[0])
    } else {
        None
    };

    let mut store = match load_store(storage_handle) {
        Ok(store) => store,
        Err(_) => return EXIT_STORE,
    };

    // Public control channel; handed to clients by whoever spawns us.
    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return EXIT_STORE,
    };
    // The launcher receives the reply channel via LaunchStoredImageReply
    // semantics only when launched through the manager; standalone spawns can
    // duplicate public.second before we close our side.
    let _ = public.second;

    loop {
        if lifecycle_stop_requested(bootstrap) {
            persist_store(storage_handle, &store);
            let _ = rt::handle_close(public.first);
            if let Some(handle) = storage_handle {
                let _ = rt::handle_close(handle);
            }
            return EXIT_OK;
        }

        let mut request = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut request) {
            Ok(()) => {
                let mut scratch = RequestScratch::new();
                let mut response = RawMessage::empty(0);
                protocol::handle_request(&mut store, &request, &mut response, &mut scratch);
                if response.tag != 0 {
                    let _ = rt::channel_send(request.handles[0], &response);
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

fn load_store(storage_handle: Option<rt::Handle>) -> Result<AccountStore, ()> {
    let Some(storage_handle) = storage_handle else {
        return Ok(AccountStore::seed_defaults());
    };
    let mut bytes = [0u8; MAX_STORE_BYTES];
    let loaded = match rt::storage_open(storage_handle, ACCOUNTS_PATH) {
        Ok((blob, _)) => {
            let mut offset = 0usize;
            while offset < bytes.len() {
                match rt::storage_read(blob, offset, &mut bytes[offset..]) {
                    Ok(0) => break,
                    Ok(read) => offset += read,
                    Err(_) => {
                        let _ = rt::storage_blob_close(blob);
                        return Err(());
                    }
                }
            }
            let _ = rt::storage_blob_close(blob);
            offset
        }
        Err(rt::Error::NotFound) => 0,
        Err(_) => return Err(()),
    };
    if loaded == 0 {
        // First boot: seed defaults so the next mutation persists them.
        return Ok(AccountStore::seed_defaults());
    }
    let text = str::from_utf8(&bytes[..loaded]).map_err(|_| ())?;
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
