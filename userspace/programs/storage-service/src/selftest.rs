use rt::{
    STORAGE_MOUNT_FLAG_PERSISTENT, STORAGE_MOUNT_FLAG_WRITABLE, STORAGE_ROOT_AUTHORITY,
    StorageEntryKind, StorageMountKind, StorageStatus,
};
use serviceos_userspace_runtime as rt;

use crate::{
    BlobSession, DirectorySession, EntrySlot, INITIAL_FILE_CAPACITY, MAX_BLOB_SESSIONS,
    MAX_DIRECTORY_SESSIONS, MAX_MUTABLE_ENTRIES, MountTable, MutableEntry, PersistentStore,
    fsck::{
        CODE_DUPLICATE_PATH, CODE_INVALID_PATH, CODE_KIND_SLASH_MISMATCH, CODE_SNAPSHOT_CHECKSUM,
        FsckReport, full_scan,
    },
    index::SearchIndex,
    path::find_mutable_entry,
    persistent::{
        load_persistent_slot, parse_header, persist_state, release_blob_session,
        release_directory_session, release_mutable_entry,
    },
    root::try_rename_entry,
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

    // 8. Gated e2e witness: server-side rename/move (0x527/0x528 wire ops).
    // Fully inert unless the image was built with SERVICEOS_E2E_STORAGE=1.
    rename_move_probe(mounts, entries, mutable_entries);

    // 9. Gated e2e witness: corruption-seeded fsck repair (0x525/0x526).
    fsck_repair_probe(
        mounts,
        entries,
        mutable_entries,
        persistent_store.as_deref_mut(),
    );

    // 10. Gated e2e witness: unmount/re-mount capability (0x51b/0x51c wire
    // ops) with a full snapshot re-parse between the two.
    unmount_capability_probe(
        mounts,
        entries,
        mutable_entries,
        blob_sessions,
        directory_sessions,
        persistent_store.as_deref_mut(),
    );

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
    let Some(dir_index) = mutable_entries.iter().position(|entry| !entry.occupied) else {
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
    let Some(file_index) = mutable_entries.iter().position(|entry| !entry.occupied) else {
        release_mutable_entry(&mut mutable_entries[dir_index]);
        return false;
    };
    mutable_entries[file_index] = MutableEntry::empty();
    {
        let slot = &mut mutable_entries[file_index];
        slot.kind = StorageEntryKind::File;
        slot.path[..PROBE_FILE.len()].copy_from_slice(PROBE_FILE);
        slot.path_len = PROBE_FILE.len();
        slot.data_handle = rt::memory_create(PROBE_PAYLOAD.len().max(INITIAL_FILE_CAPACITY), true)
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

/// Gated e2e witness for the server-side rename/move core
/// (`root.rs try_rename_entry`, the shared implementation behind the
/// 0x527/0x528 wire ops). Covers: same-directory rename, cross-directory
/// move, destination-collision rejection, directory rename carrying a
/// child subtree, and index consistency after the renames (old paths =
/// miss, new path = hit through the real search planner). Emits
/// `E2E storage.rename PASS|FAIL` only when the image was built with
/// SERVICEOS_E2E_STORAGE=1; otherwise returns before any output or
/// mutation so default boots stay byte-identical (smoke/no-probe-defaults).
fn rename_move_probe(
    mounts: &mut MountTable,
    entries: &[EntrySlot],
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
) {
    if !matches!(option_env!("SERVICEOS_E2E_STORAGE"), Some("1")) {
        return;
    }
    const PROBE_FILE: &[u8] = b"data/e2e-rename.txt";
    const PROBE_RENAMED: &[u8] = b"data/e2e-renamed.txt";
    const PROBE_DIR: &[u8] = b"data/e2e-move/";
    const PROBE_MOVED: &[u8] = b"data/e2e-move/moved.txt";
    const PROBE_DIR_MOVED: &[u8] = b"data/e2e-moved-dir/";
    const PROBE_TREE_FILE: &[u8] = b"data/e2e-moved-dir/moved.txt";
    const PROBE_COLLIDE: &[u8] = b"data/e2e-collide.txt";
    const PROBE_PAYLOAD: &[u8] = b"storage-rename-e2e";

    fn fail(step: u32, status: u32) {
        logf_args(format_args!(
            "E2E storage.rename FAIL step={step} status={status}"
        ));
    }
    // Best-effort residue removal on any path through the probe so the
    // final persist never records probe leftovers.
    fn cleanup(mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES]) {
        for path in [
            PROBE_TREE_FILE,
            PROBE_MOVED,
            PROBE_RENAMED,
            PROBE_COLLIDE,
            PROBE_FILE,
            PROBE_DIR_MOVED,
            PROBE_DIR,
        ] {
            if let Some(index) = find_mutable_entry(mutable_entries, path) {
                release_mutable_entry(&mut mutable_entries[index]);
            }
        }
    }

    let tick = rt::monotonic_now().unwrap_or(0);

    // 1. CREATE the working set under the writable `data/` namespace.
    let created = create_rename_probe_entry(
        mutable_entries,
        PROBE_FILE,
        StorageEntryKind::File,
        PROBE_PAYLOAD,
    ) && create_rename_probe_entry(
        mutable_entries,
        PROBE_DIR,
        StorageEntryKind::Directory,
        &[],
    ) && create_rename_probe_entry(
        mutable_entries,
        PROBE_COLLIDE,
        StorageEntryKind::File,
        PROBE_PAYLOAD,
    );
    if !created {
        fail(1, StorageStatus::NotFound as u32);
        return;
    }

    // The index must be built from the live entries before try_rename_entry
    // mutates it (lazy-build contract, same as the mutation probe).
    let mut probe_index = crate::index::SearchIndex::new();
    probe_index.ensure_built(entries, mutable_entries, tick);

    // 2. RENAME same-directory (data/e2e-rename.txt -> data/e2e-renamed.txt).
    let status = try_rename_entry(
        mounts,
        entries,
        mutable_entries,
        &mut probe_index,
        PROBE_FILE,
        PROBE_RENAMED,
        tick.wrapping_add(1),
    );
    if status != StorageStatus::Ok {
        cleanup(mutable_entries);
        fail(2, status as u32);
        return;
    }

    // 3. MOVE cross-directory (data/e2e-renamed.txt -> data/e2e-move/moved.txt).
    let status = try_rename_entry(
        mounts,
        entries,
        mutable_entries,
        &mut probe_index,
        PROBE_RENAMED,
        PROBE_MOVED,
        tick.wrapping_add(2),
    );
    if status != StorageStatus::Ok {
        cleanup(mutable_entries);
        fail(3, status as u32);
        return;
    }

    // 4. Destination collision must be rejected without overwrite.
    let status = try_rename_entry(
        mounts,
        entries,
        mutable_entries,
        &mut probe_index,
        PROBE_MOVED,
        PROBE_COLLIDE,
        tick.wrapping_add(3),
    );
    if status != StorageStatus::AlreadyExists {
        cleanup(mutable_entries);
        fail(4, status as u32);
        return;
    }

    // 5. RENAME a directory with a child: the subtree must follow
    // (data/e2e-move/ -> data/e2e-moved-dir/, child becomes
    // data/e2e-moved-dir/moved.txt).
    let status = try_rename_entry(
        mounts,
        entries,
        mutable_entries,
        &mut probe_index,
        PROBE_DIR,
        PROBE_DIR_MOVED,
        tick.wrapping_add(4),
    );
    if status != StorageStatus::Ok {
        cleanup(mutable_entries);
        fail(5, status as u32);
        return;
    }

    // 6. The child followed the tree rename with its payload intact.
    let child_bytes = find_mutable_entry(mutable_entries, PROBE_TREE_FILE)
        .map(|index| mutable_entries[index].data_len)
        .unwrap_or(0);
    if child_bytes != PROBE_PAYLOAD.len() {
        cleanup(mutable_entries);
        fail(6, child_bytes as u32);
        return;
    }

    // 7. Index consistency: every pre-rename path is a miss, both
    // post-rename paths are hits, and the real search planner resolves
    // the moved file by name exactly once.
    let snapshot = probe_index.snapshot();
    let old_gone = [PROBE_FILE, PROBE_RENAMED, PROBE_MOVED, PROBE_DIR]
        .iter()
        .all(|path| !snapshot.iter().any(|entry| entry.path() == *path));
    if !old_gone {
        cleanup(mutable_entries);
        fail(7, StorageStatus::NotFound as u32);
        return;
    }
    let new_present = snapshot.iter().any(|entry| entry.path() == PROBE_DIR_MOVED)
        && snapshot.iter().any(|entry| entry.path() == PROBE_TREE_FILE);
    if !new_present {
        cleanup(mutable_entries);
        fail(7, StorageStatus::InvalidPath as u32);
        return;
    }
    let plan = crate::index::plan_search(snapshot, b"data/", &[b"moved.txt"], 0, u64::MAX, 0, 100);
    let index_hit = plan.len == 1 && snapshot[plan.order[0]].path() == PROBE_TREE_FILE;
    if !index_hit {
        cleanup(mutable_entries);
        fail(8, plan.len as u32);
        return;
    }

    cleanup(mutable_entries);
    logf("E2E storage.rename PASS create=3 rename=3 collide-denied=1 index-hit=1");
}

/// Allocate one probe entry directly in the mutable table (same layer the
/// IPC create handlers compose). Files get a private memory blob preloaded
/// with `payload`.
fn create_rename_probe_entry(
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
    path: &[u8],
    kind: StorageEntryKind,
    payload: &[u8],
) -> bool {
    let Some(slot_index) = mutable_entries.iter().position(|entry| !entry.occupied) else {
        return false;
    };
    mutable_entries[slot_index] = MutableEntry::empty();
    let slot = &mut mutable_entries[slot_index];
    slot.kind = kind;
    slot.path[..path.len()].copy_from_slice(path);
    slot.path_len = path.len();
    slot.persistent = true;
    if kind == StorageEntryKind::File {
        slot.data_handle = rt::memory_create(payload.len().max(INITIAL_FILE_CAPACITY), true)
            .unwrap_or(rt::INVALID_HANDLE);
        slot.data_capacity = payload.len().max(INITIAL_FILE_CAPACITY);
        if rt::memory_write(slot.data_handle, 0, payload).is_ok() {
            slot.data_len = payload.len();
        }
    }
    slot.occupied = true;
    true
}

/// Release every entry still carrying `path` (duplicates included).
fn release_all(mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES], path: &[u8]) {
    while let Some(index) = find_mutable_entry(mutable_entries, path) {
        release_mutable_entry(&mut mutable_entries[index]);
    }
}

fn fsck_report_has(report: &FsckReport, code: u8) -> bool {
    report.iter().any(|finding| finding.code == code)
}

/// Gated e2e witness for the corruption-seeded fsck repair flow (0x525/0x526
/// wire ops, root-authority gated). Seeds three provably-bad in-memory
/// entries (duplicate path, absolute invalid path, kind/slash mismatch) plus
/// one flipped byte inside the active snapshot slot's checksummed record
/// region, then proves: the dry scan lists every finding, apply=true drops
/// exactly the three entries and re-persists a clean snapshot, a second
/// flush reclaims the corrupted stale slot, and a post-repair full scan plus
/// load+search roundtrip are intact. Emits `E2E storage.fsck PASS|FAIL` only
/// when the image was built with SERVICEOS_E2E_STORAGE=1; otherwise returns
/// before any output or mutation so default boots stay byte-identical.
fn fsck_repair_probe(
    mounts: &mut MountTable,
    entries: &[EntrySlot],
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
    mut persistent_store: Option<&mut PersistentStore>,
) {
    if !matches!(option_env!("SERVICEOS_E2E_STORAGE"), Some("1")) {
        return;
    }
    if persistent_store.is_none() {
        return;
    }
    const PROBE_FILE: &[u8] = b"data/e2e-fsck.txt";
    const PROBE_PAYLOAD: &[u8] = b"storage-fsck-e2e";
    const PROBE_INVALID: &[u8] = b"/e2e-abs.txt";
    const PROBE_BAD_DIR: &[u8] = b"data/e2e-baddir";

    fn fail(step: u32, status: u32) {
        logf_args(format_args!(
            "E2E storage.fsck FAIL step={step} status={status}"
        ));
    }
    fn cleanup(mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES]) {
        release_all(mutable_entries, PROBE_FILE);
        release_all(mutable_entries, PROBE_INVALID);
        release_all(mutable_entries, PROBE_BAD_DIR);
    }

    let tick = rt::monotonic_now().unwrap_or(0);

    // 1. Baseline: one healthy probe file persisted, full scan (memory plus
    // both snapshot slots) clean.
    if !create_rename_probe_entry(
        mutable_entries,
        PROBE_FILE,
        StorageEntryKind::File,
        PROBE_PAYLOAD,
    ) {
        fail(1, StorageStatus::NotFound as u32);
        return;
    }
    if persist_state(persistent_store.as_deref_mut(), mounts, mutable_entries).is_err() {
        cleanup(mutable_entries);
        fail(1, StorageStatus::Busy as u32);
        return;
    }
    let baseline = full_scan(
        persistent_store.as_deref_mut(),
        mounts,
        mutable_entries,
        false,
    );
    if !baseline.is_clean() {
        cleanup(mutable_entries);
        fail(2, baseline.errors);
        return;
    }

    // 2. Seed memory corruption: duplicate path, absolute invalid path,
    // kind/slash mismatch — never persisted, repairable in place.
    let seeded = create_rename_probe_entry(
        mutable_entries,
        PROBE_FILE,
        StorageEntryKind::File,
        PROBE_PAYLOAD,
    ) && create_rename_probe_entry(
        mutable_entries,
        PROBE_INVALID,
        StorageEntryKind::File,
        PROBE_PAYLOAD,
    ) && create_rename_probe_entry(
        mutable_entries,
        PROBE_BAD_DIR,
        StorageEntryKind::Directory,
        &[],
    );
    if !seeded {
        cleanup(mutable_entries);
        fail(3, StorageStatus::NotFound as u32);
        return;
    }

    // 3. Dry scan lists every seeded finding class.
    let dry = full_scan(
        persistent_store.as_deref_mut(),
        mounts,
        mutable_entries,
        false,
    );
    let listed = fsck_report_has(&dry, CODE_DUPLICATE_PATH)
        && fsck_report_has(&dry, CODE_INVALID_PATH)
        && fsck_report_has(&dry, CODE_KIND_SLASH_MISMATCH);
    if !listed {
        cleanup(mutable_entries);
        fail(4, dry.errors);
        return;
    }

    // 4. Seed snapshot corruption: flip the last byte of the last entry
    // record — checksummed metadata parsed by nothing else, so the only
    // possible finding is the checksum mismatch.
    let seed_status = seed_snapshot_checksum_flip(persistent_store.as_deref_mut());
    if seed_status != 0 {
        cleanup(mutable_entries);
        fail(5, seed_status);
        return;
    }

    // 5. Dry scan reports the checksum defect on top of the seeds.
    let dry = full_scan(
        persistent_store.as_deref_mut(),
        mounts,
        mutable_entries,
        false,
    );
    if !fsck_report_has(&dry, CODE_SNAPSHOT_CHECKSUM) {
        cleanup(mutable_entries);
        fail(6, dry.errors);
        return;
    }

    // 6. Apply repairs: exactly the three bad entries drop, the checksum
    // finding is carried in the same report, and the cleaned snapshot is
    // re-persisted.
    let applied = full_scan(
        persistent_store.as_deref_mut(),
        mounts,
        mutable_entries,
        true,
    );
    if applied.dropped != 3 || !fsck_report_has(&applied, CODE_SNAPSHOT_CHECKSUM) {
        cleanup(mutable_entries);
        fail(7, applied.dropped);
        return;
    }

    // 7. Reclaim the corrupted stale slot: one more flush rotates back onto
    // it so both slots verify clean afterwards.
    if persist_state(persistent_store.as_deref_mut(), mounts, mutable_entries).is_err() {
        cleanup(mutable_entries);
        fail(8, StorageStatus::Busy as u32);
        return;
    }
    let after = full_scan(
        persistent_store.as_deref_mut(),
        mounts,
        mutable_entries,
        false,
    );
    if !after.is_clean() {
        cleanup(mutable_entries);
        fail(9, after.errors);
        return;
    }

    // 8. Integrity roundtrip: payload bytes intact and the rebuilt search
    // index resolves the repaired file exactly once.
    let len = find_mutable_entry(mutable_entries, PROBE_FILE)
        .map(|index| mutable_entries[index].data_len)
        .unwrap_or(0);
    let mut probe_index = SearchIndex::new();
    probe_index.ensure_built(entries, mutable_entries, tick);
    let plan = crate::index::plan_search(
        probe_index.snapshot(),
        b"data/",
        &[b"e2e-fsck.txt"],
        0,
        u64::MAX,
        0,
        100,
    );
    let index_hit = plan.len == 1 && probe_index.snapshot()[plan.order[0]].path() == PROBE_FILE;
    if len != PROBE_PAYLOAD.len() || !index_hit {
        cleanup(mutable_entries);
        fail(10, len as u32);
        return;
    }

    logf("E2E storage.fsck PASS seeded=3 checksum=1 dropped=3 reclaimed=1 clean=1 index-hit=1");
}

/// Flip one checksummed-but-unparsed byte inside the active snapshot slot's
/// entry-record region. Returns 0 on success, nonzero step status otherwise.
fn seed_snapshot_checksum_flip(mut store: Option<&mut PersistentStore>) -> u32 {
    let Some(store) = store.as_deref_mut() else {
        return 1;
    };
    let slot_base = store.active_slot * store.slot_blocks;
    let mut block = [0u8; crate::BLOCK_BUFFER_BYTES];
    if rt::block_device_read(
        store.handle,
        slot_base as u64,
        &mut block[..store.block_size],
    )
    .is_err()
    {
        return 2;
    }
    let Some((_, entry_count, _, records_offset, _, _, _)) =
        parse_header(store, store.active_slot, &block, store.block_size)
    else {
        return 3;
    };
    if entry_count == 0 {
        return 4;
    }
    let target = records_offset + entry_count * crate::PERSISTENT_RECORD_BYTES - 1;
    let (block_index, byte_index) = (target / store.block_size, target % store.block_size);
    if rt::block_device_read(
        store.handle,
        (slot_base + block_index) as u64,
        &mut block[..store.block_size],
    )
    .is_err()
    {
        return 5;
    }
    block[byte_index] ^= 0xff;
    if rt::block_device_write(
        store.handle,
        (slot_base + block_index) as u64,
        &block[..store.block_size],
    )
    .is_err()
    {
        return 6;
    }
    0
}

/// Gated e2e witness for the unmount/re-mount capability contract
/// (root.rs try_unmount behind the 0x51b/0x51c wire ops). Proves the
/// capability gates (wrong authority Denied, unknown prefix NotFound, boot
/// root anchored), a clean unmount of the persistent `data/` namespace, the
/// dual-slot generation chain, and a simulated abrupt end — the whole
/// snapshot is re-parsed from the block device through the boot restore
/// path, payload bytes verified, then `data/` remounts and the search index
/// resolves the file again. Emits `E2E storage.unmount PASS|FAIL` only when
/// the image was built with SERVICEOS_E2E_STORAGE=1; otherwise returns
/// before any output or mutation so default boots stay byte-identical.
fn unmount_capability_probe(
    mounts: &mut MountTable,
    entries: &[EntrySlot],
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
    blob_sessions: &mut [BlobSession; MAX_BLOB_SESSIONS],
    directory_sessions: &mut [DirectorySession; MAX_DIRECTORY_SESSIONS],
    mut persistent_store: Option<&mut PersistentStore>,
) {
    if !matches!(option_env!("SERVICEOS_E2E_STORAGE"), Some("1")) {
        return;
    }
    if persistent_store.is_none() {
        return;
    }
    const PROBE_FILE: &[u8] = b"data/e2e-fsck.txt";
    const PROBE_PAYLOAD: &[u8] = b"storage-fsck-e2e";

    fn fail(step: u32, status: u32) {
        logf_args(format_args!(
            "E2E storage.unmount FAIL step={step} status={status}"
        ));
    }

    let authority = STORAGE_ROOT_AUTHORITY;

    // 1. Wrong authority must be rejected.
    let denied = try_unmount(
        mounts,
        mutable_entries,
        blob_sessions,
        directory_sessions,
        persistent_store.as_deref_mut(),
        b"data/",
        authority.wrapping_add(1),
    );
    if denied != StorageStatus::Denied {
        release_all(mutable_entries, PROBE_FILE);
        fail(1, denied as u32);
        return;
    }

    // 2. Unknown prefix answers NotFound.
    let not_found = try_unmount(
        mounts,
        mutable_entries,
        blob_sessions,
        directory_sessions,
        persistent_store.as_deref_mut(),
        b"e2e-absent/",
        authority,
    );
    if not_found != StorageStatus::NotFound {
        release_all(mutable_entries, PROBE_FILE);
        fail(2, not_found as u32);
        return;
    }

    // 3. The boot root anchors the namespace and can never be removed.
    let anchored = try_unmount(
        mounts,
        mutable_entries,
        blob_sessions,
        directory_sessions,
        persistent_store.as_deref_mut(),
        b"",
        authority,
    );
    if anchored != StorageStatus::Denied {
        release_all(mutable_entries, PROBE_FILE);
        fail(3, anchored as u32);
        return;
    }

    // 4. Unmount the persistent data/ namespace (no open handles by now).
    let status = try_unmount(
        mounts,
        mutable_entries,
        blob_sessions,
        directory_sessions,
        persistent_store.as_deref_mut(),
        b"data/",
        authority,
    );
    if status != StorageStatus::Ok || rt::storage_find_mount_by_path(mounts, b"data/").is_some() {
        release_all(mutable_entries, PROBE_FILE);
        fail(4, status as u32);
        return;
    }

    // 5. Simulated abrupt end: check the dual-slot generation chain, then
    // re-parse the whole snapshot from the block device through the boot
    // restore path. The unmount persist dropped the data/ mount while the
    // persistent entries ride along in the snapshot.
    let Some(store) = persistent_store.as_deref_mut() else {
        release_all(mutable_entries, PROBE_FILE);
        fail(5, StorageStatus::NotFound as u32);
        return;
    };
    let mut header = [0u8; crate::BLOCK_BUFFER_BYTES];
    let mut generations = [0u64; 2];
    let mut parsed = true;
    for slot in 0..2usize {
        if rt::block_device_read(
            store.handle,
            (slot * store.slot_blocks) as u64,
            &mut header[..store.block_size],
        )
        .is_err()
        {
            parsed = false;
            break;
        }
        match parse_header(store, slot, &header, store.block_size) {
            Some((generation, _, _, _, _, _, _)) => generations[slot] = generation,
            None => {
                parsed = false;
                break;
            }
        }
    }
    if !parsed
        || crate::fsck::chain_regression(
            generations[store.active_slot],
            generations[store.active_slot ^ 1],
        )
    {
        release_all(mutable_entries, PROBE_FILE);
        fail(5, 1);
        return;
    }
    let reloaded = matches!(
        load_persistent_slot(store, store.active_slot, mounts, mutable_entries),
        Ok(Some(_))
    );
    if !reloaded || rt::storage_find_mount_by_path(mounts, b"data/").is_some() {
        release_all(mutable_entries, PROBE_FILE);
        fail(5, 2);
        return;
    }
    let mut payload = [0u8; PROBE_PAYLOAD.len()];
    let restored = find_mutable_entry(mutable_entries, PROBE_FILE)
        .map(|index| {
            let entry = &mutable_entries[index];
            entry.data_len == PROBE_PAYLOAD.len()
                && rt::memory_read(entry.data_handle, 0, &mut payload)
                    .map(|read| read == PROBE_PAYLOAD.len() && payload.as_slice() == PROBE_PAYLOAD)
                    .unwrap_or(false)
        })
        .unwrap_or(false);
    if !restored {
        release_all(mutable_entries, PROBE_FILE);
        fail(5, 3);
        return;
    }

    // 6. Remount data/ and prove the search index resolves the file again.
    let remounted = rt::storage_mount_add(
        mounts,
        b"data/",
        StorageMountKind::Persistent,
        STORAGE_MOUNT_FLAG_WRITABLE | STORAGE_MOUNT_FLAG_PERSISTENT,
        authority,
    );
    if let Err(status) = remounted {
        release_all(mutable_entries, PROBE_FILE);
        fail(6, status as u32);
        return;
    }
    let tick = rt::monotonic_now().unwrap_or(0);
    let mut probe_index = SearchIndex::new();
    probe_index.ensure_built(entries, mutable_entries, tick);
    let plan = crate::index::plan_search(
        probe_index.snapshot(),
        b"data/",
        &[b"e2e-fsck.txt"],
        0,
        u64::MAX,
        0,
        100,
    );
    let index_hit = plan.len == 1 && probe_index.snapshot()[plan.order[0]].path() == PROBE_FILE;
    if !index_hit {
        release_all(mutable_entries, PROBE_FILE);
        fail(6, plan.len as u32);
        return;
    }

    // 7. Best-effort residue removal so the boot's final persist stays clean.
    release_all(mutable_entries, PROBE_FILE);
    logf("E2E storage.unmount PASS denied=2 notfound=1 unmount=1 reparse=1 remount=1 index-hit=1");
}
