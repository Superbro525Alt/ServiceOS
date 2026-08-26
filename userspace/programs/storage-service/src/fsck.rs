use rt::{
    Handle, RawMessage, STORAGE_MOUNT_TABLE_MAX, STORAGE_MOUNT_PATH_MAX, STORAGE_ROOT_AUTHORITY,
    StorageEntryKind, StorageMountKind, StorageStatus,
};
use serviceos_userspace_runtime as rt;

use crate::{
    MAX_MUTABLE_ENTRIES, MAX_STORAGE_PATH, MountTable, MutableEntry, PersistentStore,
    index::SearchIndex,
    path::resolve_mount,
    persistent::{PERSISTENT_VERSION_V1, parse_header, persist_state, read_record_span},
};

pub(crate) const FSCK_REQUEST_TAG: u32 = 0x525;
pub(crate) const FSCK_REPLY_TAG: u32 = 0x526;

pub(crate) const FSCK_MAX_FINDINGS: usize = 16;
const FSCK_PATH_BYTES: usize = 48;
const ORIGIN_MOUNT_BASE: u16 = 1000;

pub(crate) const CODE_ORPHANED_ENTRY: u8 = 1;
pub(crate) const CODE_DUPLICATE_PATH: u8 = 2;
pub(crate) const CODE_INVALID_PATH: u8 = 3;
pub(crate) const CODE_KIND_SLASH_MISMATCH: u8 = 4;
pub(crate) const CODE_EMPTY_PATH: u8 = 5;
pub(crate) const CODE_MOUNT_DUPLICATE: u8 = 6;
pub(crate) const CODE_MOUNT_PATH: u8 = 7;
pub(crate) const CODE_SNAPSHOT_HEADER: u8 = 8;
pub(crate) const CODE_SNAPSHOT_CHECKSUM: u8 = 9;
pub(crate) const CODE_SNAPSHOT_RECORD: u8 = 10;
pub(crate) const CODE_GENERATION_REGRESSION: u8 = 11;

const SEV_ERROR: u8 = 1;
const SEV_WARNING: u8 = 2;

#[derive(Clone, Copy)]
pub(crate) struct FsckFinding {
    severity: u8,
    code: u8,
    origin: u16,
    path_len: usize,
    path: [u8; FSCK_PATH_BYTES],
}

impl FsckFinding {
    const fn empty() -> Self {
        Self {
            severity: 0,
            code: 0,
            origin: 0,
            path_len: 0,
            path: [0; FSCK_PATH_BYTES],
        }
    }

    fn severity_label(&self) -> &'static str {
        match self.severity {
            SEV_ERROR => "E",
            _ => "W",
        }
    }

    pub(crate) fn code_label(&self) -> &'static str {
        code_label(self.code)
    }

    fn path_bytes(&self) -> &[u8] {
        &self.path[..self.path_len]
    }
}

#[derive(Clone, Copy)]
pub(crate) struct FsckReport {
    errors: u32,
    warnings: u32,
    dropped: u32,
    rebuilt_index: bool,
    findings: [FsckFinding; FSCK_MAX_FINDINGS],
    count: usize,
    truncated: bool,
}

impl FsckReport {
    pub(crate) const fn new() -> Self {
        Self {
            errors: 0,
            warnings: 0,
            dropped: 0,
            rebuilt_index: false,
            findings: [FsckFinding::empty(); FSCK_MAX_FINDINGS],
            count: 0,
            truncated: false,
        }
    }

    fn push(&mut self, severity: u8, code: u8, origin: u16, path: &[u8]) {
        match severity {
            SEV_ERROR => self.errors += 1,
            _ => self.warnings += 1,
        }
        if self.count >= FSCK_MAX_FINDINGS {
            self.truncated = true;
            return;
        }
        let mut stored = [0u8; FSCK_PATH_BYTES];
        let copy_len = path.len().min(FSCK_PATH_BYTES);
        stored[..copy_len].copy_from_slice(&path[..copy_len]);
        self.findings[self.count] = FsckFinding {
            severity,
            code,
            origin,
            path_len: copy_len,
            path: stored,
        };
        self.count += 1;
    }

    fn error(&mut self, code: u8, origin: u16, path: &[u8]) {
        self.push(SEV_ERROR, code, origin, path);
    }

    fn warning(&mut self, code: u8, origin: u16, path: &[u8]) {
        self.push(SEV_WARNING, code, origin, path);
    }

    pub(crate) fn is_clean(&self) -> bool {
        self.errors == 0 && self.warnings == 0
    }

    pub(crate) fn counts(&self) -> (u32, u32, u32, bool) {
        (self.errors, self.warnings, self.dropped, self.rebuilt_index)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &FsckFinding> {
        self.findings[..self.count].iter()
    }
}

fn code_label(code: u8) -> &'static str {
    match code {
        CODE_ORPHANED_ENTRY => "orphan",
        CODE_DUPLICATE_PATH => "duplicate",
        CODE_INVALID_PATH => "bad-path",
        CODE_KIND_SLASH_MISMATCH => "kind-slash",
        CODE_EMPTY_PATH => "empty-path",
        CODE_MOUNT_DUPLICATE => "mount-dup",
        CODE_MOUNT_PATH => "mount-path",
        CODE_SNAPSHOT_HEADER => "snap-header",
        CODE_SNAPSHOT_CHECKSUM => "snap-checksum",
        CODE_SNAPSHOT_RECORD => "snap-record",
        CODE_GENERATION_REGRESSION => "gen-regress",
        _ => "unknown",
    }
}

pub(crate) fn entry_path_violation(path: &[u8], kind: StorageEntryKind) -> Option<u8> {
    if path.is_empty() {
        return Some(CODE_EMPTY_PATH);
    }
    if path[0] == b'/' {
        return Some(CODE_INVALID_PATH);
    }
    match kind {
        StorageEntryKind::Directory => {
            if !path.ends_with(b"/") {
                return Some(CODE_KIND_SLASH_MISMATCH);
            }
        }
        StorageEntryKind::File => {
            if path.ends_with(b"/") {
                return Some(CODE_KIND_SLASH_MISMATCH);
            }
        }
    }
    let core = match kind {
        StorageEntryKind::Directory => &path[..path.len() - 1],
        StorageEntryKind::File => path,
    };
    if core.is_empty() || !rt::storage_relative_components_valid(core) {
        return Some(CODE_INVALID_PATH);
    }
    None
}

struct EntryVerdict {
    violation: Option<u8>,
    orphaned: bool,
    duplicate_of: bool,
}

fn judge_entry(
    mounts: &MountTable,
    mutable_entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
    index: usize,
) -> EntryVerdict {
    let entry = &mutable_entries[index];
    let path = &entry.path[..entry.path_len];
    let violation = entry_path_violation(path, entry.kind);
    let orphaned = violation.is_none() && resolve_mount(mounts, path).is_none();
    let duplicate_of = violation.is_none()
        && mutable_entries[..index].iter().any(|other| {
            other.occupied
                && other.path_len == entry.path_len
                && other.path[..other.path_len] == *path
        });
    EntryVerdict {
        violation,
        orphaned,
        duplicate_of,
    }
}

pub(crate) fn scan_memory(
    mounts: &MountTable,
    mutable_entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
    report: &mut FsckReport,
) {
    for (index, entry) in mutable_entries.iter().enumerate() {
        if !entry.occupied {
            continue;
        }
        let verdict = judge_entry(mounts, mutable_entries, index);
        let path = &entry.path[..entry.path_len];
        if let Some(code) = verdict.violation {
            report.error(code, index as u16, path);
        }
        if verdict.duplicate_of {
            report.error(CODE_DUPLICATE_PATH, index as u16, path);
        }
        if verdict.orphaned {
            report.warning(CODE_ORPHANED_ENTRY, index as u16, path);
        }
    }
    for i in 0..mounts.len() {
        let mount = &mounts[i];
        if !mount.occupied {
            continue;
        }
        let path = &mount.path[..mount.path_len];
        if rt::storage_validate_mount_path(path).is_err() {
            report.warning(CODE_MOUNT_PATH, i as u16, path);
        }
        if mounts[..i].iter().any(|other| {
            other.occupied && other.path_len == mount.path_len && other.path[..other.path_len] == *path
        }) {
            report.error(CODE_MOUNT_DUPLICATE, i as u16, path);
        }
    }
}

pub(crate) fn scan_memory_report(
    mounts: &MountTable,
    mutable_entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
) -> FsckReport {
    let mut report = FsckReport::new();
    scan_memory(mounts, mutable_entries, &mut report);
    report
}

pub(crate) fn collect_drop_plan(
    mounts: &MountTable,
    mutable_entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
) -> ([bool; MAX_MUTABLE_ENTRIES], usize) {
    let mut plan = [false; MAX_MUTABLE_ENTRIES];
    let mut count = 0usize;
    for index in 0..mutable_entries.len() {
        if !mutable_entries[index].occupied {
            continue;
        }
        let verdict = judge_entry(mounts, mutable_entries, index);
        if verdict.violation.is_some() || verdict.orphaned || verdict.duplicate_of {
            plan[index] = true;
            count += 1;
        }
    }
    (plan, count)
}

pub(crate) fn apply_repairs(
    mounts: &MountTable,
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
    report: &mut FsckReport,
) -> u32 {
    let (plan, expected) = collect_drop_plan(mounts, mutable_entries);
    for index in 0..mutable_entries.len() {
        if !plan[index] {
            continue;
        }
        let entry = &mut mutable_entries[index];
        if entry.data_handle != rt::INVALID_HANDLE {
            let _ = rt::handle_close(entry.data_handle);
        }
        report.dropped += 1;
        *entry = MutableEntry::empty();
    }
    debug_assert_eq!(report.dropped, expected as u32);
    report.rebuilt_index = true;
    report.dropped
}

#[derive(Clone, Copy)]
pub(crate) struct SnapshotFields {
    pub(crate) magic_ok: bool,
    pub(crate) version: u32,
    pub(crate) entry_count: usize,
    pub(crate) mount_count: usize,
    pub(crate) records_offset: usize,
    pub(crate) data_offset: usize,
    pub(crate) total_bytes: usize,
}

pub(crate) fn validate_snapshot_fields(
    fields: &SnapshotFields,
    max_entries: usize,
    max_mounts: usize,
    block_size: usize,
    slot_bytes: usize,
) -> Option<u8> {
    if !fields.magic_ok
        || (fields.version != crate::PERSISTENT_VERSION && fields.version != PERSISTENT_VERSION_V1)
    {
        return Some(CODE_SNAPSHOT_HEADER);
    }
    if fields.entry_count > max_entries || fields.mount_count > max_mounts {
        return Some(CODE_SNAPSHOT_HEADER);
    }
    if fields.records_offset < block_size
        || fields.total_bytes == 0
        || fields.total_bytes > slot_bytes
        || fields.data_offset < fields.records_offset
            + fields.entry_count * crate::PERSISTENT_RECORD_BYTES
            + fields.mount_count * crate::MOUNT_RECORD_BYTES
    {
        return Some(CODE_SNAPSHOT_HEADER);
    }
    None
}

pub(crate) fn chain_regression(active_generation: u64, stale_generation: u64) -> bool {
    stale_generation > active_generation
}

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

pub(crate) struct SnapshotChecksummer {
    hash: u64,
}

impl SnapshotChecksummer {
    pub(crate) const fn new() -> Self {
        Self {
            hash: FNV_OFFSET_BASIS,
        }
    }

    pub(crate) fn feed(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.hash ^= u64::from(*byte);
            self.hash = self.hash.wrapping_mul(FNV_PRIME);
        }
    }

    pub(crate) fn finish(&self) -> u64 {
        self.hash
    }
}

#[cfg(test)]
pub(crate) fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut summer = SnapshotChecksummer::new();
    summer.feed(bytes);
    summer.finish()
}

pub(crate) fn header_checksum_input(header_block: &[u8]) -> &[u8] {
    &header_block[..52.min(header_block.len())]
}

pub(crate) fn boot_quick_scan(
    mounts: &MountTable,
    mutable_entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
    persistent_store: Option<&PersistentStore>,
) -> FsckReport {
    let mut report = FsckReport::new();
    scan_memory(mounts, mutable_entries, &mut report);
    let (errors, warnings, _, _) = report.counts();
    let _ = rt::write_logf(
        "storage",
        format_args!(
            "fsck quick-scan errors={} warnings={} findings={}{} mode=warn-only generation={}",
            errors,
            warnings,
            report.count,
            if report.truncated { "+" } else { "" },
            persistent_store.map(|store| store.generation).unwrap_or(0),
        ),
    );
    for finding in report.iter() {
        let _ = rt::write_logf(
            "storage",
            format_args!(
                "fsck finding sev={} code={} origin={} path={}",
                finding.severity_label(),
                finding.code_label(),
                finding.origin,
                core::str::from_utf8(finding.path_bytes()).unwrap_or("?"),
            ),
        );
    }
    report
}

pub(crate) fn full_scan(
    persistent_store: Option<&mut PersistentStore>,
    mounts: &MountTable,
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
    apply: bool,
) -> FsckReport {
    let mut report = FsckReport::new();
    scan_memory(mounts, mutable_entries, &mut report);
    if let Some(store) = persistent_store.as_ref() {
        scan_snapshot_slots(store, mounts, &mut report);
    }
    if apply {
        apply_repairs(mounts, mutable_entries, &mut report);
        if let Some(store) = persistent_store {
            if persist_state(Some(store), mounts, mutable_entries).is_err() {
                report.error(CODE_SNAPSHOT_HEADER, u16::MAX, b"persist-after-repair");
            }
        }
    }
    log_report("scan", apply, &report);
    report
}

fn log_report(action: &str, apply: bool, report: &FsckReport) {
    let (errors, warnings, dropped, rebuilt) = report.counts();
    let mode = match (action, apply) {
        ("scan", _) => action,
        (_, true) => "repair",
        (_, false) => "dry-run",
    };
    let _ = rt::write_logf(
        "storage",
        format_args!(
            "fsck {} errors={} warnings={} dropped={} rebuilt-index={} findings={}{}",
            mode,
            errors,
            warnings,
            dropped,
            rebuilt as u32,
            report.count,
            if report.truncated { "+" } else { "" },
        ),
    );
}

fn scan_snapshot_slots(
    store: &PersistentStore,
    mounts: &MountTable,
    report: &mut FsckReport,
) {
    let block_size = store.block_size;
    let slot_bytes = store.slot_blocks * block_size;
    let mut generations = [None; 2];

    for slot in 0..2usize {
        let mut block = [0u8; crate::BLOCK_BUFFER_BYTES];
        if block_size > block.len() {
            return;
        }
        if rt::block_device_read(
            store.handle,
            (slot * store.slot_blocks) as u64,
            &mut block[..block_size],
        )
        .is_err()
        {
            report.error(CODE_SNAPSHOT_HEADER, slot as u16, b"slot-unreadable");
            continue;
        }
        if block[..block_size].iter().all(|byte| *byte == 0) {
            continue;
        }
        let magic_ok = block[..crate::PERSISTENT_MAGIC.len()] == crate::PERSISTENT_MAGIC;
        let version = u32::from_le_bytes(block[8..12].try_into().unwrap());
        let Some((generation, entry_count, mount_count, records_offset, data_offset, total_bytes, stored_checksum)) =
            parse_header(store, slot, &block, block_size)
        else {
            report.error(CODE_SNAPSHOT_HEADER, slot as u16, b"unparseable");
            continue;
        };
        generations[slot] = Some(generation);
        let fields = SnapshotFields {
            magic_ok,
            version,
            entry_count,
            mount_count,
            records_offset,
            data_offset,
            total_bytes,
        };
        if let Some(code) = validate_snapshot_fields(
            &fields,
            MAX_MUTABLE_ENTRIES,
            STORAGE_MOUNT_TABLE_MAX,
            block_size,
            slot_bytes,
        ) {
            report.error(code, slot as u16, b"layout");
            continue;
        }
        if stored_checksum != 0 {
            let computed =
                recompute_snapshot_checksum(store, slot, &block, records_offset, data_offset);
            if computed != stored_checksum {
                report.error(CODE_SNAPSHOT_CHECKSUM, slot as u16, b"metadata");
            }
        }
        verify_slot_records(store, slot, &fields, mounts, report);
    }

    let active = generations[store.active_slot];
    let stale = generations[store.active_slot ^ 1];
    if let (Some(active_gen), Some(stale_gen)) = (active, stale) {
        if chain_regression(active_gen, stale_gen) {
            report.error(CODE_GENERATION_REGRESSION, store.active_slot as u16, b"slots");
        }
    }
}

fn recompute_snapshot_checksum(
    store: &PersistentStore,
    slot: usize,
    header_block: &[u8; crate::BLOCK_BUFFER_BYTES],
    records_offset: usize,
    data_offset: usize,
) -> u64 {
    let block_size = store.block_size;
    let mut summer = SnapshotChecksummer::new();
    summer.feed(header_checksum_input(header_block));
    let mut cursor = records_offset;
    while cursor < data_offset {
        let block_start = (cursor / block_size) * block_size;
        let mut block = [0u8; crate::BLOCK_BUFFER_BYTES];
        if rt::block_device_read(
            store.handle,
            (slot * store.slot_blocks + block_start / block_size) as u64,
            &mut block[..block_size],
        )
        .is_err()
        {
            return 0;
        }
        let chunk_end = block_start + block_size;
        let end = data_offset.min(chunk_end);
        summer.feed(&block[block_start % block_size..end - block_start]);
        cursor = end.max(chunk_end);
    }
    summer.finish()
}

fn verify_slot_records(
    store: &PersistentStore,
    slot: usize,
    fields: &SnapshotFields,
    mounts: &MountTable,
    report: &mut FsckReport,
) {
    for record_index in 0..fields.entry_count {
        let record_offset = fields.records_offset + record_index * crate::PERSISTENT_RECORD_BYTES;
        let Ok(record) = read_record_span(store, slot, record_offset) else {
            report.error(CODE_SNAPSHOT_RECORD, record_index as u16, b"unreadable");
            continue;
        };
        if record[0] == 0 {
            continue;
        }
        if record[0] > 1 {
            report.error(CODE_SNAPSHOT_RECORD, record_index as u16, b"occupied-flag");
            continue;
        }
        let kind = match record[1] {
            0 => StorageEntryKind::File,
            1 => StorageEntryKind::Directory,
            _ => {
                report.error(CODE_SNAPSHOT_RECORD, record_index as u16, b"kind");
                continue;
            }
        };
        let path_len = u16::from_le_bytes(record[2..4].try_into().unwrap()) as usize;
        if path_len == 0 || path_len > MAX_STORAGE_PATH {
            report.error(CODE_SNAPSHOT_RECORD, record_index as u16, b"path-len");
            continue;
        }
        let path = &record[24..24 + path_len];
        if let Some(code) = entry_path_violation(path, kind) {
            report.error(code, record_index as u16, path);
        }
        if resolve_mount(mounts, path).is_none() {
            report.warning(CODE_ORPHANED_ENTRY, record_index as u16, path);
        }
        if kind == StorageEntryKind::File {
            let data_len = u64::from_le_bytes(record[8..16].try_into().unwrap()) as usize;
            let file_offset = u64::from_le_bytes(record[16..24].try_into().unwrap()) as usize;
            if file_offset < fields.data_offset
                || file_offset.saturating_add(data_len) > fields.total_bytes
            {
                report.error(CODE_SNAPSHOT_RECORD, record_index as u16, path);
            }
        }
    }

    let mounts_base = fields.records_offset + fields.entry_count * crate::PERSISTENT_RECORD_BYTES;
    let mut seen_paths: [[u8; STORAGE_MOUNT_PATH_MAX]; STORAGE_MOUNT_TABLE_MAX] =
        [[0; STORAGE_MOUNT_PATH_MAX]; STORAGE_MOUNT_TABLE_MAX];
    let mut seen_lens = [0usize; STORAGE_MOUNT_TABLE_MAX];
    let mut seen_count = 0usize;
    for record_index in 0..fields.mount_count {
        let origin = ORIGIN_MOUNT_BASE + record_index as u16;
        let record_offset = mounts_base + record_index * crate::MOUNT_RECORD_BYTES;
        let Ok(record) = read_record_span(store, slot, record_offset) else {
            report.error(CODE_SNAPSHOT_RECORD, origin, b"unreadable");
            continue;
        };
        if record[0] == 0 {
            continue;
        }
        let kind_raw = u32::from_le_bytes(record[4..8].try_into().unwrap());
        let known_kind = [
            StorageMountKind::Boot as u32,
            StorageMountKind::Persistent as u32,
            StorageMountKind::Ephemeral as u32,
            StorageMountKind::Temp as u32,
        ]
        .contains(&kind_raw);
        if !known_kind {
            report.error(CODE_SNAPSHOT_RECORD, origin, b"mount-kind");
            continue;
        }
        let path_len = u16::from_le_bytes(record[2..4].try_into().unwrap()) as usize;
        if path_len > STORAGE_MOUNT_PATH_MAX {
            report.error(CODE_SNAPSHOT_RECORD, origin, b"mount-path-len");
            continue;
        }
        let path = &record[24..24 + path_len];
        if rt::storage_validate_mount_path(path).is_err() {
            report.warning(CODE_MOUNT_PATH, origin, path);
        }
        if seen_lens[..seen_count]
            .iter()
            .zip(seen_paths[..seen_count].iter())
            .any(|(seen_len, seen)| *seen_len == path_len && seen[..path_len] == *path)
        {
            report.error(CODE_MOUNT_DUPLICATE, origin, path);
        } else if seen_count < STORAGE_MOUNT_TABLE_MAX {
            seen_paths[seen_count][..path_len].copy_from_slice(path);
            seen_lens[seen_count] = path_len;
            seen_count += 1;
        }
    }
}

pub(crate) fn handle_fsck_request(
    mounts: &mut MountTable,
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
    search_index: &mut SearchIndex,
    persistent_store: Option<&mut PersistentStore>,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 2 || message.handle_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let apply = message.words[0] == 1;
    let authority = message.words[1];

    if authority != STORAGE_ROOT_AUTHORITY {
        send_fsck_reply(reply_handle, StorageStatus::Denied, &FsckReport::new(), false);
        return Ok(());
    }

    let report = full_scan(persistent_store, mounts, mutable_entries, apply);

    if apply && report.rebuilt_index {
        search_index.mark_dirty();
    }

    send_fsck_reply(reply_handle, StorageStatus::Ok, &report, apply);
    Ok(())
}

fn send_fsck_reply(reply_handle: Handle, status: StorageStatus, report: &FsckReport, apply: bool) {
    let (errors, warnings, dropped, rebuilt) = report.counts();
    let mut reply = RawMessage::empty(FSCK_REPLY_TAG);
    reply.word_count = 6;
    reply.words[0] = status as u32 as u64;
    reply.words[1] = u64::from(errors);
    reply.words[2] = u64::from(warnings);
    reply.words[3] = u64::from(dropped);
    reply.words[4] = u64::from(rebuilt);
    reply.words[5] = u64::from(apply);
    for (slot, finding) in report.iter().enumerate() {
        let word_index = 6 + slot;
        if word_index >= rt::IPC_MAX_WORDS {
            break;
        }
        reply.word_count += 1;
        reply.words[word_index] = (u64::from(finding.severity) << 56)
            | (u64::from(finding.code) << 48)
            | u64::from(finding.origin);
    }
    crate::util::send_reply_and_close(reply_handle, &reply);
}

#[cfg(test)]
mod tests {
    use super::*;
    use rt::{STORAGE_MOUNT_FLAG_PERSISTENT, STORAGE_MOUNT_FLAG_WRITABLE};

    fn seeded_mounts() -> MountTable {
        let mut mounts = [rt::StorageMount::empty(); STORAGE_MOUNT_TABLE_MAX];
        let defaults: [(&[u8], StorageMountKind, u64); 3] = [
            (
                b"home/",
                StorageMountKind::Persistent,
                STORAGE_MOUNT_FLAG_WRITABLE | STORAGE_MOUNT_FLAG_PERSISTENT,
            ),
            (
                b"state/",
                StorageMountKind::Persistent,
                STORAGE_MOUNT_FLAG_WRITABLE | STORAGE_MOUNT_FLAG_PERSISTENT,
            ),
            (
                b"tmp/",
                StorageMountKind::Ephemeral,
                STORAGE_MOUNT_FLAG_WRITABLE,
            ),
        ];
        for (slot, (path, kind, flags)) in mounts.iter_mut().zip(defaults.iter()) {
            assert!(slot.install(path, *kind, *flags, STORAGE_ROOT_AUTHORITY).is_ok());
        }
        mounts
    }

    fn file_entry(path: &[u8]) -> MutableEntry {
        let mut entry = MutableEntry::empty();
        entry.kind = StorageEntryKind::File;
        entry.path[..path.len()].copy_from_slice(path);
        entry.path_len = path.len();
        entry.persistent = true;
        entry.occupied = true;
        entry
    }

    fn dir_entry(path: &[u8]) -> MutableEntry {
        let mut entry = file_entry(path);
        entry.kind = StorageEntryKind::Directory;
        entry
    }

    fn contains_code(report: &FsckReport, code: u8) -> bool {
        report.iter().any(|finding| finding.code == code)
    }

    #[test]
    fn clean_fixture_reports_no_findings() {
        let mounts = seeded_mounts();
        let mut entries = [MutableEntry::empty(); MAX_MUTABLE_ENTRIES];
        entries[0] = file_entry(b"home/a.txt");
        entries[1] = dir_entry(b"home/docs/");
        entries[2] = file_entry(b"state/x");
        entries[3] = file_entry(b"tmp/session-1");
        entries[4] = dir_entry(b"home/deep/nested/");
        let report = scan_memory_report(&mounts, &entries);
        assert!(report.is_clean(), "expected clean");
        assert_eq!(report.iter().count(), 0);
    }

    #[test]
    fn scan_detects_every_corruption_class() {
        let mounts = seeded_mounts();
        let mut entries = [MutableEntry::empty(); MAX_MUTABLE_ENTRIES];
        entries[0] = file_entry(b"orphan/a.bin");
        entries[1] = file_entry(b"home/dup.txt");
        entries[2] = file_entry(b"home/dup.txt");
        entries[3] = file_entry(b"/abs.txt");
        entries[4] = dir_entry(b"home/notdir");
        entries[5] = file_entry(b"home/isdir/");
        entries[6] = file_entry(b"home/bad/../name");
        let report = scan_memory_report(&mounts, &entries);
        assert_eq!(report.errors, 5);
        assert_eq!(report.warnings, 1);
        assert!(contains_code(&report, CODE_ORPHANED_ENTRY));
        assert!(contains_code(&report, CODE_DUPLICATE_PATH));
        assert!(contains_code(&report, CODE_KIND_SLASH_MISMATCH));
        assert!(contains_code(&report, CODE_INVALID_PATH));
    }

    #[test]
    fn orphan_needs_missing_mount_prefix() {
        let mounts = seeded_mounts();
        let mut entries = [MutableEntry::empty(); MAX_MUTABLE_ENTRIES];
        entries[0] = file_entry(b"home/kept.txt");
        entries[1] = file_entry(b"gone/lost.txt");
        let report = scan_memory_report(&mounts, &entries);
        assert_eq!(report.warnings, 1);
        assert_eq!(report.errors, 0);
    }

    #[test]
    fn repair_drops_bad_records_and_is_idempotent() {
        let mut mounts = seeded_mounts();
        assert!(mounts[3]
            .install(
                b"extra/",
                StorageMountKind::Temp,
                STORAGE_MOUNT_FLAG_WRITABLE,
                STORAGE_ROOT_AUTHORITY
            )
            .is_ok());

        let mut entries = [MutableEntry::empty(); MAX_MUTABLE_ENTRIES];
        entries[0] = file_entry(b"home/keep.txt");
        entries[1] = file_entry(b"gone/orphan.bin");
        entries[4] = file_entry(b"home/keep.txt");
        entries[5] = dir_entry(b"home/oops");
        entries[6] = dir_entry(b"extra/nested/");
        entries[7] = MutableEntry::empty();

        let mut report = FsckReport::new();
        scan_memory(&mounts, &entries, &mut report);
        let dropped_first = apply_repairs(&mounts, &mut entries, &mut report);
        assert_eq!(dropped_first, 3, "orphan + duplicate + kind-slash");

        assert!(find_entry(&entries, b"home/keep.txt"));
        assert!(find_entry(&entries, b"extra/nested/"));
        assert!(!find_entry(&entries, b"gone/orphan.bin"));
        assert!(!find_entry(&entries, b"home/oops"));
        assert!(report.rebuilt_index);
        assert_eq!(report.dropped, 3);

        let second_scan = scan_memory_report(&mounts, &entries);
        assert!(second_scan.is_clean(), "post-repair scan must be clean");
        let mut verify_report = FsckReport::new();
        assert_eq!(apply_repairs(&mounts, &mut entries, &mut verify_report), 0);
        assert!(verify_report.is_clean());
    }

    #[test]
    fn repair_preserves_survivor_records_in_place() {
        let mounts = seeded_mounts();
        let mut entries = [MutableEntry::empty(); MAX_MUTABLE_ENTRIES];
        entries[0] = file_entry(b"home/first");
        entries[1] = file_entry(b"nope/orphan");
        entries[2] = file_entry(b"home/third");
        let mut report = FsckReport::new();
        scan_memory(&mounts, &entries, &mut report);
        apply_repairs(&mounts, &mut entries, &mut report);
        assert!(entries[0].occupied);
        assert!(!entries[1].occupied);
        assert!(entries[2].occupied);
        assert_eq!(&entries[0].path[..entries[0].path_len], b"home/first");
        assert_eq!(&entries[2].path[..entries[2].path_len], b"home/third");
    }

    fn find_entry(entries: &[MutableEntry; MAX_MUTABLE_ENTRIES], path: &[u8]) -> bool {
        entries.iter().any(|entry| {
            entry.occupied && entry.path_len == path.len() && entry.path[..entry.path_len] == *path
        })
    }

    fn base_fields() -> SnapshotFields {
        SnapshotFields {
            magic_ok: true,
            version: crate::PERSISTENT_VERSION,
            entry_count: 2,
            mount_count: 1,
            records_offset: 512,
            data_offset: 512 + 2 * 128 + 1 * 128,
            total_bytes: 4096,
        }
    }

    #[test]
    fn snapshot_layout_validator_accepts_current_and_v1_formats() {
        assert_eq!(
            validate_snapshot_fields(&base_fields(), 128, 16, 512, 8192),
            None
        );
        let mut v1 = base_fields();
        v1.version = PERSISTENT_VERSION_V1;
        v1.mount_count = 0;
        v1.data_offset = 512 + 2 * 128;
        assert_eq!(validate_snapshot_fields(&v1, 128, 16, 512, 8192), None);
    }

    #[test]
    fn snapshot_layout_validator_rejects_each_field_class() {
        let mut bad_magic = base_fields();
        bad_magic.magic_ok = false;
        assert_eq!(
            validate_snapshot_fields(&bad_magic, 128, 16, 512, 8192),
            Some(CODE_SNAPSHOT_HEADER)
        );
        let mut bad_version = base_fields();
        bad_version.version = 99;
        assert_eq!(
            validate_snapshot_fields(&bad_version, 128, 16, 512, 8192),
            Some(CODE_SNAPSHOT_HEADER)
        );
        let mut many_entries = base_fields();
        many_entries.entry_count = 129;
        assert_eq!(
            validate_snapshot_fields(&many_entries, 128, 16, 512, 8192),
            Some(CODE_SNAPSHOT_HEADER)
        );
        let mut many_mounts = base_fields();
        many_mounts.mount_count = 17;
        assert_eq!(
            validate_snapshot_fields(&many_mounts, 128, 16, 512, 8192),
            Some(CODE_SNAPSHOT_HEADER)
        );
        let mut oversized = base_fields();
        oversized.total_bytes = 8193;
        assert_eq!(
            validate_snapshot_fields(&oversized, 128, 16, 512, 8192),
            Some(CODE_SNAPSHOT_HEADER)
        );
        let mut empty = base_fields();
        empty.total_bytes = 0;
        assert_eq!(
            validate_snapshot_fields(&empty, 128, 16, 512, 8192),
            Some(CODE_SNAPSHOT_HEADER)
        );
        let mut overlapping = base_fields();
        overlapping.data_offset = 512 + 2 * 128;
        assert_eq!(
            validate_snapshot_fields(&overlapping, 128, 16, 512, 8192),
            Some(CODE_SNAPSHOT_HEADER)
        );
        let mut misaligned_meta = base_fields();
        misaligned_meta.records_offset = 256;
        assert_eq!(
            validate_snapshot_fields(&misaligned_meta, 128, 16, 512, 8192),
            Some(CODE_SNAPSHOT_HEADER)
        );
    }

    #[test]
    fn generation_chain_flags_stale_slot_ahead_of_active() {
        assert!(!chain_regression(9, 8));
        assert!(!chain_regression(9, 0));
        assert!(!chain_regression(9, 9));
        assert!(chain_regression(5, 6));
    }

    #[test]
    fn fnv1a64_matches_known_vectors_and_incremental_split() {
        assert_eq!(fnv1a64(b""), 0xcbf29ce484222325);
        assert_eq!(fnv1a64(b"a"), 0xaf63dc4c8601ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x85944171f73967e8);
        let mut summer = SnapshotChecksummer::new();
        summer.feed(b"foo");
        summer.feed(b"bar");
        assert_eq!(summer.finish(), fnv1a64(b"foobar"));
    }

    #[test]
    fn header_checksum_input_excludes_checksum_field_bytes() {
        let mut block = [0u8; 512];
        block[..8].copy_from_slice(b"SOSPSTR1");
        block[50] = 0xAA;
        block[55] = 0xBB;
        let input = header_checksum_input(&block);
        assert_eq!(input.len(), 52);
        assert_eq!(input[50], 0xAA);
        let mut without_field = [0u8; 512];
        without_field[..52].copy_from_slice(&input);
        assert_eq!(fnv1a64(input), fnv1a64(&without_field[..52]));
    }

    #[test]
    fn report_caps_findings_without_losing_counts() {
        let mounts = seeded_mounts();
        let mut entries = [MutableEntry::empty(); MAX_MUTABLE_ENTRIES];
        for index in 0..FSCK_MAX_FINDINGS + 4 {
            entries[index] = file_entry(b"gone/overflow");
        }
        let report = scan_memory_report(&mounts, &entries);
        assert_eq!(report.warnings as usize, FSCK_MAX_FINDINGS + 4);
        assert_eq!(
            report.errors as usize,
            FSCK_MAX_FINDINGS + 4 - 1,
            "all-but-first are duplicates"
        );
        assert_eq!(report.iter().count(), FSCK_MAX_FINDINGS);
        assert!(report.truncated);
    }
}
