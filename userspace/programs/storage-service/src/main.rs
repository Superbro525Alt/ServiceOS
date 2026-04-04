#![no_std]
#![no_main]

mod blob;
mod directory;
mod lifecycle;
mod path;
mod persistent;
mod root;
mod state;
mod util;

pub(crate) use persistent::{
    initialize_persistent_store, release_blob_session, release_directory_session,
    release_mutable_entry,
};
pub(crate) use state::*;

use serviceos_bundle::{parse_boot_store_entry, parse_boot_store_header, BootStoreHeader};
use serviceos_userspace_runtime as rt;
use rt::{ControlTag, RawMessage, ServiceId};

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf501;
    }
    if startup.tag != ControlTag::Startup as u32 || startup.handle_count < 1 || startup.word_count < 5 {
        return 0xf502;
    }

    let bootstore_handle = startup.handles[0];
    let block_handle = if startup.handle_count >= 2 {
        startup.handles[1]
    } else {
        rt::INVALID_HANDLE
    };
    let bootstore_len = startup.words[4] as usize;
    let mut header_bytes = [0u8; BootStoreHeader::encoded_len()];
    if rt::memory_read(bootstore_handle, 0, &mut header_bytes).ok() != Some(header_bytes.len()) {
        return 0xf503;
    }
    let header = match parse_boot_store_header(&header_bytes) {
        Ok(header) => header,
        Err(_) => return 0xf504,
    };
    let entry_count = header.entry_count as usize;
    if entry_count > MAX_BOOTSTORE_ENTRIES {
        return 0xf505;
    }

    let mut table_bytes = [0u8; MAX_BOOTSTORE_ENTRIES * BOOT_ENTRY_BYTES];
    let entry_table_len = entry_count * BOOT_ENTRY_BYTES;
    if rt::memory_read(
        bootstore_handle,
        header.entry_table_offset as usize,
        &mut table_bytes[..entry_table_len],
    )
    .ok()
        != Some(entry_table_len)
    {
        return 0xf506;
    }

    let mut entries = [EntrySlot::empty(); MAX_BOOTSTORE_ENTRIES];
    for index in 0..entry_count {
        let start = index * BOOT_ENTRY_BYTES;
        let end = start + BOOT_ENTRY_BYTES;
        let record = match parse_boot_store_entry(&table_bytes[start..end]) {
            Ok(record) => record,
            Err(_) => return 0xf507,
        };
        let Some(kind) = record.kind() else {
            return 0xf508;
        };
        if record.data_offset as usize + record.data_len as usize > bootstore_len {
            let _ = rt::write_logf(
                "storage",
                format_args!(
                    "invalid entry index={} offset={} len={} total={}",
                    index,
                    record.data_offset,
                    record.data_len,
                    bootstore_len,
                ),
            );
            return 0xf509;
        }
        let path_len = record.path_len as usize;
        entries[index].kind = kind;
        entries[index].data_offset = record.data_offset as usize;
        entries[index].data_len = record.data_len as usize;
        entries[index].path_len = path_len;
        entries[index].path[..path_len].copy_from_slice(&record.path[..path_len]);
    }

    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return 0xf50a,
    };
    if rt::register_service(bootstrap, ServiceId::Storage, public.second).is_err() {
        return 0xf50b;
    }
    let _ = rt::handle_close(public.second);

    let mut blob_sessions = [BlobSession::empty(); MAX_BLOB_SESSIONS];
    let mut directory_sessions = [DirectorySession::empty(); MAX_DIRECTORY_SESSIONS];
    let mut mutable_entries = [MutableEntry::empty(); MAX_MUTABLE_ENTRIES];
    let mut persistent_store = if block_handle != rt::INVALID_HANDLE {
        match initialize_persistent_store(block_handle, &mut mutable_entries) {
            Ok(store) => store,
            Err(error) => {
                let _ = rt::write_logf(
                    "storage",
                    format_args!("persistent backing unavailable error={:?}", error),
                );
                None
            }
        }
    } else {
        None
    };
    let _ = rt::write_logf(
        "storage",
        format_args!(
            "mounted boot-store entries={} bytes={} persistent={}",
            entry_count,
            bootstore_len,
            persistent_store.is_some(),
        ),
    );

    loop {
        match lifecycle::poll_lifecycle(bootstrap) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(_) => return 0xf50c,
        }

        let mut root_request = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut root_request) {
            Ok(()) => {
                if root::handle_root_request(
                    &entries[..entry_count],
                    &mut mutable_entries,
                    &mut blob_sessions,
                    &mut directory_sessions,
                    persistent_store.as_mut(),
                    &root_request,
                )
                .is_err()
                {
                    return 0xf50d;
                }
            }
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => return 0xf50e,
        }

        for session in &mut blob_sessions {
            if !session.occupied {
                continue;
            }
            let mut request = RawMessage::empty(0);
            match rt::channel_receive_nonblocking(session.endpoint, &mut request) {
                Ok(()) => {
                    if blob::handle_blob_request(
                        bootstore_handle,
                        &mut mutable_entries,
                        persistent_store.as_mut(),
                        session,
                        &request,
                    )
                    .is_err()
                    {
                        return 0xf50f;
                    }
                }
                Err(rt::Error::QueueEmpty) => {}
                Err(_) => release_blob_session(session),
            }
        }

        for session in &mut directory_sessions {
            if !session.occupied {
                continue;
            }
            let mut request = RawMessage::empty(0);
            match rt::channel_receive_nonblocking(session.endpoint, &mut request) {
                Ok(()) => {
                    if directory::handle_directory_request(
                        &entries[..entry_count],
                        &mut mutable_entries,
                        &mut blob_sessions,
                        persistent_store.as_mut(),
                        session,
                        &request,
                    )
                    .is_err()
                    {
                        return 0xf510;
                    }
                }
                Err(rt::Error::QueueEmpty) => {}
                Err(_) => release_directory_session(session),
            }
        }

        if rt::yield_current().is_err() {
            return 0xf511;
        }
    }
}
