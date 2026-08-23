use rt::{
    STORAGE_MOUNT_FLAG_PERSISTENT, STORAGE_MOUNT_FLAG_WRITABLE, STORAGE_ROOT_AUTHORITY,
    StorageEntryKind, StorageMountKind, StorageStatus,
};
use serviceos_userspace_runtime as rt;

use crate::{
    BlobSession, DirectorySession, INITIAL_FILE_CAPACITY, MAX_BLOB_SESSIONS,
    MAX_DIRECTORY_SESSIONS, MAX_MUTABLE_ENTRIES, MountTable, MutableEntry, PersistentStore,
    path::find_mutable_entry,
    persistent::{persist_state, release_blob_session},
    root::try_unmount,
};

const SELFTEST_PREFIX: &[u8] = b"data/";
const SELFTEST_TEMP_PREFIX: &[u8] = b"scratch/";
const SELFTEST_FILE: &[u8] = b"data/note.txt";
const SELFTEST_PAYLOAD: &[u8] = b"serviceos-mount-selftest";

pub(crate) fn run_boot_selftest(
    mounts: &mut MountTable,
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
    blob_sessions: &mut [BlobSession; MAX_BLOB_SESSIONS],
    directory_sessions: &mut [DirectorySession; MAX_DIRECTORY_SESSIONS],
    mut persistent_store: Option<&mut PersistentStore>,
) {
    let authority = STORAGE_ROOT_AUTHORITY;

    // 1. Explicit mount mutation (idempotent across reboots).
    let mounted = if let Some(_slot) = rt::storage_find_mount_by_path(mounts, SELFTEST_PREFIX) {
        logf("selftest mount-present restored=1");
        true
    } else {
        match rt::storage_mount_add(
            mounts,
            SELFTEST_PREFIX,
            StorageMountKind::Persistent,
            STORAGE_MOUNT_FLAG_WRITABLE | STORAGE_MOUNT_FLAG_PERSISTENT,
            authority,
        ) {
            Ok(_) => {
                logf("selftest mount-added ok");
                true
            }
            Err(status) => {
                logf_args(format_args!(
                    "selftest mount-added FAILED status={}",
                    status as u32
                ));
                false
            }
        }
    };

    // Wrong authority must be rejected (capability gate).
    if let Some(index) = rt::storage_find_mount_by_path(mounts, SELFTEST_PREFIX) {
        let gated = !rt::storage_mount_authority_ok(&mounts[index], authority.wrapping_add(1));
        logf_args(format_args!(
            "selftest authority-gate denied-ok={}",
            gated as u32
        ));
    }

    if !mounted {
        return;
    }

    // 2. File written inside the freshly mounted namespace.
    let note_len = ensure_selftest_file(mounts, mutable_entries);
    if persist_state(persistent_store.as_deref_mut(), mounts, mutable_entries).is_ok() {
        logf_args(format_args!("selftest file-written bytes={} ok", note_len));
    } else {
        logf("selftest persist FAILED");
        return;
    }

    // 3. Open handle blocks unmount.
    let held = open_selftest_blob(blob_sessions, mounts, mutable_entries);
    let busy_status = try_unmount(
        mounts,
        mutable_entries,
        blob_sessions,
        directory_sessions,
        persistent_store.as_deref_mut(),
        SELFTEST_PREFIX,
        authority,
    );
    logf_args(format_args!(
        "selftest unmount-open refused={} (status={})",
        busy_status == StorageStatus::Busy,
        busy_status as u32
    ));
    if held {
        for session in blob_sessions.iter_mut() {
            if session.occupied && session.path_len == SELFTEST_FILE.len() {
                release_blob_session(session);
                break;
            }
        }
    }

    // 4. Unmount succeeds once handles are closed.
    let closed_status = try_unmount(
        mounts,
        mutable_entries,
        blob_sessions,
        directory_sessions,
        persistent_store.as_deref_mut(),
        SELFTEST_PREFIX,
        authority,
    );
    logf_args(format_args!(
        "selftest unmount-closed ok={} (status={})",
        closed_status == StorageStatus::Ok,
        closed_status as u32
    ));

    // 5. Remount: persistent backend keeps the file bytes.
    match rt::storage_mount_add(
        mounts,
        SELFTEST_PREFIX,
        StorageMountKind::Persistent,
        STORAGE_MOUNT_FLAG_WRITABLE | STORAGE_MOUNT_FLAG_PERSISTENT,
        authority,
    ) {
        Ok(_) => {
            let survived = find_mutable_entry(mutable_entries, SELFTEST_FILE)
                .map(|index| mutable_entries[index].data_len)
                .unwrap_or(0);
            logf_args(format_args!(
                "selftest remount persists-bytes={} ok={}",
                survived,
                survived == SELFTEST_PAYLOAD.len()
            ));
        }
        Err(status) => logf_args(format_args!(
            "selftest remount FAILED status={}",
            status as u32
        )),
    }

    // 6. Second backend instance: in-memory Temp namespace composes beside the rest.
    match rt::storage_mount_add(
        mounts,
        SELFTEST_TEMP_PREFIX,
        StorageMountKind::Temp,
        STORAGE_MOUNT_FLAG_WRITABLE,
        authority,
    ) {
        Ok(_) => {
            let temp_path = b"scratch/session.tmp";
            if let Some(slot) = mutable_entries.iter_mut().find(|entry| !entry.occupied) {
                *slot = MutableEntry::empty();
                slot.kind = StorageEntryKind::File;
                slot.path[..temp_path.len()].copy_from_slice(temp_path);
                slot.path_len = temp_path.len();
                slot.persistent = false;
                slot.data_handle =
                    rt::memory_create(INITIAL_FILE_CAPACITY, true).unwrap_or(rt::INVALID_HANDLE);
                slot.data_capacity = INITIAL_FILE_CAPACITY;
                slot.data_len = 0;
                slot.occupied = true;
            }
            let temp_mounts = mounts.iter().filter(|m| m.occupied).count();
            logf_args(format_args!(
                "selftest multi-backend mounts={} composed=1",
                temp_mounts
            ));
            // Unmounting a Temp namespace drops its contents.
            let purged_status = try_unmount(
                mounts,
                mutable_entries,
                blob_sessions,
                directory_sessions,
                persistent_store.as_deref_mut(),
                SELFTEST_TEMP_PREFIX,
                authority,
            );
            let purged = find_mutable_entry(mutable_entries, temp_path).is_none();
            logf_args(format_args!(
                "selftest temp-unmount ok={} purged={} (status={})",
                purged_status == StorageStatus::Ok,
                purged,
                purged_status as u32
            ));
        }
        Err(status) => logf_args(format_args!(
            "selftest temp-mount skipped status={}",
            status as u32
        )),
    }

    let _ = persist_state(persistent_store, mounts, mutable_entries);
}

fn ensure_selftest_file(
    mounts: &MountTable,
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
) -> usize {
    let index = match find_mutable_entry(mutable_entries, SELFTEST_FILE) {
        Some(index) => index,
        None => {
            let Some(slot_index) = mutable_entries.iter().position(|entry| !entry.occupied) else {
                return 0;
            };
            mutable_entries[slot_index] = MutableEntry::empty();
            mutable_entries[slot_index].kind = StorageEntryKind::File;
            mutable_entries[slot_index].path[..SELFTEST_FILE.len()].copy_from_slice(SELFTEST_FILE);
            mutable_entries[slot_index].path_len = SELFTEST_FILE.len();
            mutable_entries[slot_index].persistent = true;
            mutable_entries[slot_index].data_handle =
                rt::memory_create(INITIAL_FILE_CAPACITY, true).unwrap_or(rt::INVALID_HANDLE);
            mutable_entries[slot_index].data_capacity = INITIAL_FILE_CAPACITY;
            mutable_entries[slot_index].occupied = true;
            slot_index
        }
    };
    let _ = mounts;
    let entry = &mut mutable_entries[index];
    if entry.data_capacity < SELFTEST_PAYLOAD.len() {
        let _ = rt::handle_close(entry.data_handle);
        entry.data_handle = rt::INVALID_HANDLE;
        entry.data_handle =
            rt::memory_create(SELFTEST_PAYLOAD.len().max(INITIAL_FILE_CAPACITY), true)
                .unwrap_or(rt::INVALID_HANDLE);
        entry.data_capacity = SELFTEST_PAYLOAD.len().max(INITIAL_FILE_CAPACITY);
    }
    if entry.data_handle != rt::INVALID_HANDLE
        && rt::memory_write(entry.data_handle, 0, SELFTEST_PAYLOAD).is_ok()
    {
        entry.data_len = SELFTEST_PAYLOAD.len();
    }
    entry.data_len
}

fn open_selftest_blob(
    blob_sessions: &mut [BlobSession; MAX_BLOB_SESSIONS],
    mounts: &MountTable,
    mutable_entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
) -> bool {
    let Some(index) = find_mutable_entry(mutable_entries, SELFTEST_FILE) else {
        return false;
    };
    let Some(session) = blob_sessions.iter_mut().find(|session| !session.occupied) else {
        return false;
    };
    let pair = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return false,
    };
    *session = crate::stamp_blob_session(
        mounts,
        pair.first,
        SELFTEST_FILE,
        crate::BlobSource::Mutable,
        0,
        mutable_entries[index].data_len,
        mutable_entries[index].data_handle,
        index,
        true,
    );
    let _ = rt::handle_close(pair.second);
    true
}

fn logf(message: &str) {
    let _ = rt::write_logf("storage", format_args!("{}", message));
}

fn logf_args(args: core::fmt::Arguments) {
    let _ = rt::write_logf("storage", args);
}
