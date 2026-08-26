use rt::{
    STORAGE_MOUNT_FLAG_PERSISTENT, STORAGE_MOUNT_FLAG_WRITABLE, STORAGE_ROOT_AUTHORITY,
    StorageEntryKind, StorageMountKind, StorageStatus,
};
use serviceos_userspace_runtime as rt;

use crate::{
    BlobSession, DirectorySession, EntrySlot, INITIAL_FILE_CAPACITY, MAX_BLOB_SESSIONS,
    MAX_DIRECTORY_SESSIONS, MAX_MUTABLE_ENTRIES, MountTable, MutableEntry, PersistentStore,
    path::find_mutable_entry,
    persistent::{persist_state, release_blob_session, release_directory_session,
        release_mutable_entry},
    root::try_unmount,
};

const SELFTEST_PREFIX: &[u8] = b"data/";
const SELFTEST_TEMP_PREFIX: &[u8] = b"scratch/";
const SELFTEST_FILE: &[u8] = b"data/note.txt";
const SELFTEST_PAYLOAD: &[u8] = b"serviceos-mount-selftest";

pub(crate) fn run_boot_selftest(
    mounts: &mut MountTable,
    entries: &[EntrySlot],
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

    let mut probe_index = crate::index::SearchIndex::new();
    probe_index.ensure_built(entries, mutable_entries, 100);
    let probe_plan = crate::index::plan_search(
        probe_index.snapshot(),
        b"data/",
        &[b"note"],
        0,
        u64::MAX,
        0,
        100,
    );
    let probe_hit = if probe_plan.len > 0 {
        probe_index.snapshot()[probe_plan.order[0]].path() == SELFTEST_FILE
    } else {
        false
    };
    let probe_needle = match crate::index::StreamNeedle::new(b"mount-selftest") {
        Some(mut stream) => {
            let mut hits = 0usize;
            stream.feed(SELFTEST_PAYLOAD, |_| hits += 1);
            hits
        }
        None => 0,
    };
    let (probe_count, probe_dirty, probe_rebuild) = probe_index.stats();
    logf_args(format_args!(
        "selftest index-probe entries={} dirty={} rebuild-tick={} search-hit={} grep-lines={}",
        probe_count, probe_dirty as u32, probe_rebuild, probe_hit as u32, probe_needle
    ));

    // 2b. Mutation probe: create dir+file, index-rename, delete both —
    // exercises the same entry/index paths the files-app ops compose.
    let mutation_ok = mutation_probe(mutable_entries);
    logf_args(format_args!(
        "selftest mutation-probe create=2 rename=1 delete=2 ok={}",
        mutation_ok as u32
    ));

    let fsck_report = crate::fsck::scan_memory_report(mounts, mutable_entries);
    let (fsck_errors, fsck_warnings, _, _) = fsck_report.counts();
    logf_args(format_args!(
        "selftest fsck-report errors={} warnings={} items={} ok={}",
        fsck_errors,
        fsck_warnings,
        fsck_report.iter().count(),
        fsck_report.is_clean() as u32,
    ));

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

    // 7. Mounted namespace roots must open even without backing entries
    // (this is how the files app opens `data/` and `home/`).
    for root_path in [SELFTEST_PREFIX, b"home/" as &[u8]] {
        let opened = probe_mount_root_open(
            mounts,
            entries,
            mutable_entries,
            directory_sessions,
            root_path,
        );
        let shown = core::str::from_utf8(root_path).unwrap_or("?");
        logf_args(format_args!(
            "selftest mount-root-open {shown} ok={}",
            opened as u32
        ));
    }

    let _ = persist_state(persistent_store, mounts, mutable_entries);
}

/// Drive the real `DirectoryOpenRequest` handler for a mounted namespace root
/// and report whether it answered `Ok`.
fn probe_mount_root_open(
    mounts: &mut MountTable,
    entries: &[EntrySlot],
    mutable_entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
    directory_sessions: &mut [DirectorySession; MAX_DIRECTORY_SESSIONS],
    path: &[u8],
) -> bool {
    let Some(pair) = rt::channel_create().ok() else {
        return false;
    };
    let mut request = rt::RawMessage::empty(rt::StorageTag::DirectoryOpenRequest as u32);
    let packed = match crate::util::pack_bytes(path, &mut request.words[2..]) {
        Ok(packed) => packed,
        Err(_) => {
            let _ = rt::handle_close(pair.first);
            let _ = rt::handle_close(pair.second);
            return false;
        }
    };
    request.word_count = 2 + packed;
    request.words[0] = path.len() as u64;
    request.words[1] = 0;
    request.handle_count = 1;
    request.handles[0] = pair.second;
    request.handle_rights[0] = rt::rights::SEND;

    // The handler consumes and closes `pair.second` when it sends its reply.
    if crate::root::handle_directory_open_request(
        mounts,
        entries,
        mutable_entries,
        directory_sessions,
        &request,
    )
    .is_err()
    {
        return false;
    }

    let mut reply = rt::RawMessage::empty(0);
    let received = rt::channel_receive_blocking(pair.first, &mut reply).is_ok();
    let _ = rt::handle_close(pair.first);
    if !received {
        return false;
    }
    if reply.handle_count > 0 {
        // We own the received session-endpoint capability; drop it.
        let _ = rt::handle_close(reply.handles[0]);
    }
    let opened = reply.word_count >= 1 && reply.words[0] == StorageStatus::Ok as u32 as u64;
    if let Some(session) = directory_sessions
        .iter_mut()
        .find(|session| session.occupied && &session.path[..session.path_len] == path)
    {
        release_directory_session(session);
    }
    opened
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

/// Direct-entry mutation probe (same layer the IPC handlers compose):
/// create a directory and a file, rename the file through the search
/// index helper, then delete both entries. Returns overall success.
fn mutation_probe(mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES]) -> bool {
    const PROBE_DIR: &[u8] = b"selfprobe/";
    const PROBE_FILE: &[u8] = b"selfprobe/note.txt";
    const PROBE_RENAMED: &[u8] = b"selfprobe/renamed.txt";
    const PROBE_PAYLOAD: &[u8] = b"mutation-probe";

    let tick = rt::monotonic_now().unwrap_or(0);

    // CREATE directory + file entries.
    let Some(dir_index) = mutable_entries
        .iter()
        .position(|entry| !entry.occupied)
    else {
        return false;
    };
    mutable_entries[dir_index] = MutableEntry::empty();
    {
        let slot = &mut mutable_entries[dir_index];
        slot.kind = StorageEntryKind::Directory;
        slot.path[..PROBE_DIR.len()].copy_from_slice(PROBE_DIR);
        slot.path_len = PROBE_DIR.len();
        slot.occupied = true;
    }
    let Some(file_index) = mutable_entries
        .iter()
        .position(|entry| !entry.occupied)
    else {
        release_mutable_entry(&mut mutable_entries[dir_index]);
        return false;
    };
    mutable_entries[file_index] = MutableEntry::empty();
    {
        let slot = &mut mutable_entries[file_index];
        slot.kind = StorageEntryKind::File;
        slot.path[..PROBE_FILE.len()].copy_from_slice(PROBE_FILE);
        slot.path_len = PROBE_FILE.len();
        slot.data_handle =
            rt::memory_create(PROBE_PAYLOAD.len().max(INITIAL_FILE_CAPACITY), true)
                .unwrap_or(rt::INVALID_HANDLE);
        slot.data_capacity = PROBE_PAYLOAD.len().max(INITIAL_FILE_CAPACITY);
        if rt::memory_write(slot.data_handle, 0, PROBE_PAYLOAD).is_ok() {
            slot.data_len = PROBE_PAYLOAD.len();
        }
        slot.occupied = true;
    }

    // RENAME through the index helper (wire-level composition target).
    // The index must be built from the live entries before upsert/rename
    // mutate it (lazy-build contract).
    let mut probe_index = crate::index::SearchIndex::new();
    probe_index.ensure_built(&[], mutable_entries, tick);
    probe_index.rename(PROBE_FILE, PROBE_RENAMED, tick.wrapping_add(1));
    let renamed_present = probe_index
        .snapshot()
        .iter()
        .any(|entry| entry.path() == PROBE_RENAMED);
    let old_gone = !probe_index
        .snapshot()
        .iter()
        .any(|entry| entry.path() == PROBE_FILE);
    let rename_ok = renamed_present && old_gone;

    // DELETE both entries and confirm release.
    release_mutable_entry(&mut mutable_entries[file_index]);
    release_mutable_entry(&mut mutable_entries[dir_index]);
    let deleted_ok = find_mutable_entry(mutable_entries, PROBE_FILE).is_none()
        && find_mutable_entry(mutable_entries, PROBE_RENAMED).is_none()
        && find_mutable_entry(mutable_entries, PROBE_DIR).is_none();

    rename_ok && deleted_ok
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
