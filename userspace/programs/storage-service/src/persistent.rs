use rt::{
    BlockDeviceBackend, Handle, STORAGE_MOUNT_FLAG_PERSISTENT, STORAGE_MOUNT_FLAG_WRITABLE,
    STORAGE_MOUNT_TABLE_MAX, StorageMount, StorageMountKind,
};
use serviceos_userspace_runtime as rt;

use crate::state::{
    BLOCK_BUFFER_BYTES, INITIAL_FILE_CAPACITY, MAX_MUTABLE_ENTRIES, MOUNT_RECORD_BYTES, MountTable,
    MutableEntry, PERSISTENT_MAGIC, PERSISTENT_RECORD_BYTES, PERSISTENT_VERSION, PersistentStore,
};

/// v1 snapshots carried only mutable entries; v2 adds explicit mount records.
pub(crate) const PERSISTENT_VERSION_V1: u32 = 1;

pub(crate) fn initialize_persistent_store(
    block_handle: Handle,
    mounts: &mut MountTable,
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
) -> rt::Result<Option<PersistentStore>> {
    let info = rt::block_device_info(block_handle)?;
    if info.backend != BlockDeviceBackend::VirtioPci as u32
        || info.writable == 0
        || info.block_size == 0
        || info.block_count < 16
    {
        return Ok(None);
    }
    let block_size = info.block_size as usize;
    let block_count = info.block_count as usize;
    let slot_blocks = (block_count / 2).max(8);
    let mut best_store = PersistentStore {
        handle: block_handle,
        block_size,
        slot_blocks,
        active_slot: 0,
        generation: 0,
    };
    let mut best_generation = 0u64;
    let mut loaded_any = false;

    for slot in 0..2usize {
        if let Some((generation, _stored_mounts)) =
            load_persistent_slot(&best_store, slot, mounts, mutable_entries)?
            && (!loaded_any || generation >= best_generation)
        {
            best_generation = generation;
            best_store.active_slot = slot;
            best_store.generation = generation;
            loaded_any = true;
        }
    }

    if loaded_any {
        // Re-load winning slot for both entries and mounts.
        let _ = load_persistent_slot(&best_store, best_store.active_slot, mounts, mutable_entries)?;
        let _ = rt::write_logf(
            "storage",
            format_args!(
                "mounted persistent snapshot blocks={} generation={} mounts={}",
                slot_blocks,
                best_generation,
                mounts.iter().filter(|mount| mount.occupied).count(),
            ),
        );
    } else {
        seed_mount_table(mounts);
        let _ = rt::write_logf(
            "storage",
            format_args!(
                "initialized empty persistent snapshot blocks={}",
                slot_blocks
            ),
        );
    }
    ensure_boot_root(mounts);

    Ok(Some(best_store))
}

pub(crate) fn seed_mount_table(mounts: &mut MountTable) {
    *mounts = [StorageMount::empty(); STORAGE_MOUNT_TABLE_MAX];
    const ROOT_AUTHORITY: u64 = rt::STORAGE_ROOT_AUTHORITY;
    let defaults: [(&[u8], StorageMountKind, u64); 5] = [
        (b"", StorageMountKind::Boot, 0),
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
            b"projects/",
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
        let _ = slot.install(path, *kind, *flags, ROOT_AUTHORITY);
    }
}

pub(crate) fn ensure_boot_root(mounts: &mut MountTable) {
    if rt::storage_find_mount_by_path(mounts, b"").is_none()
        && let Some(slot) = mounts.iter_mut().find(|mount| !mount.occupied)
    {
        let _ = slot.install(b"", StorageMountKind::Boot, 0, rt::STORAGE_ROOT_AUTHORITY);
    }
}

pub(crate) fn persist_state(
    persistent_store: Option<&mut PersistentStore>,
    mounts: &MountTable,
    mutable_entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
) -> rt::Result<()> {
    let Some(store) = persistent_store else {
        return Ok(());
    };
    flush_persistent_store(store, mounts, mutable_entries)
}

pub(crate) fn parse_header(
    store: &PersistentStore,
    _slot: usize,
    block: &[u8],
    block_size: usize,
) -> Option<(u64, usize, usize, usize, usize, usize, u64)> {
    if block[..PERSISTENT_MAGIC.len()] != PERSISTENT_MAGIC {
        return None;
    }
    let version = u32::from_le_bytes(block[8..12].try_into().unwrap());
    if version != PERSISTENT_VERSION && version != PERSISTENT_VERSION_V1 {
        return None;
    }
    let generation = u64::from_le_bytes(block[16..24].try_into().unwrap());
    let entry_count = u32::from_le_bytes(block[12..16].try_into().unwrap()) as usize;
    let records_offset = u64::from_le_bytes(block[24..32].try_into().unwrap()) as usize;
    let data_offset = u64::from_le_bytes(block[32..40].try_into().unwrap()) as usize;
    let total_bytes = u64::from_le_bytes(block[40..48].try_into().unwrap()) as usize;
    let mount_count = if version >= PERSISTENT_VERSION {
        u32::from_le_bytes(block[48..52].try_into().unwrap()) as usize
    } else {
        0
    };
    let stored_checksum = u64::from_le_bytes(block[52..60].try_into().unwrap());
    if entry_count > MAX_MUTABLE_ENTRIES
        || mount_count > STORAGE_MOUNT_TABLE_MAX
        || records_offset < block_size
        || total_bytes == 0
        || total_bytes > store.slot_blocks * block_size
        || data_offset
            < align_up(
                records_offset
                    + entry_count * PERSISTENT_RECORD_BYTES
                    + mount_count * MOUNT_RECORD_BYTES,
                block_size,
            )
    {
        return None;
    }
    Some((
        generation,
        entry_count,
        mount_count,
        records_offset,
        data_offset,
        total_bytes,
        stored_checksum,
    ))
}

pub(crate) fn load_persistent_slot(
    store: &PersistentStore,
    slot: usize,
    mounts: &mut MountTable,
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
) -> rt::Result<Option<(u64, usize)>> {
    let block_size = store.block_size;
    let mut block = [0u8; BLOCK_BUFFER_BYTES];
    if block_size > block.len() {
        return Ok(None);
    }
    rt::block_device_read(
        store.handle,
        (slot * store.slot_blocks) as u64,
        &mut block[..block_size],
    )?;
    let Some((generation, entry_count, mount_count, records_offset, _, _, _)) =
        parse_header(store, slot, &block, block_size)
    else {
        return Ok(None);
    };

    for entry in mutable_entries.iter_mut() {
        release_mutable_entry(entry);
    }

    for record_index in 0..entry_count {
        let record_offset = records_offset + record_index * PERSISTENT_RECORD_BYTES;
        let record = read_record_span(store, slot, record_offset)?;
        let occupied = record[0] != 0;
        if !occupied {
            continue;
        }
        let kind = match record[1] {
            0 => rt::StorageEntryKind::File,
            1 => rt::StorageEntryKind::Directory,
            _ => return Ok(None),
        };
        let path_len = u16::from_le_bytes(record[2..4].try_into().unwrap()) as usize;
        let data_len = u64::from_le_bytes(record[8..16].try_into().unwrap()) as usize;
        let file_offset = u64::from_le_bytes(record[16..24].try_into().unwrap()) as usize;
        if path_len == 0 || path_len > crate::MAX_STORAGE_PATH {
            return Ok(None);
        }
        let Some(slot_entry) = mutable_entries.iter_mut().find(|entry| !entry.occupied) else {
            return Ok(None);
        };
        *slot_entry = MutableEntry::empty();
        slot_entry.kind = kind;
        slot_entry.path_len = path_len;
        slot_entry.path[..path_len].copy_from_slice(&record[24..24 + path_len]);
        // Everything that reached the snapshot is persistent by definition.
        slot_entry.persistent = true;
        slot_entry.occupied = true;
        if kind == rt::StorageEntryKind::File {
            let capacity = data_len.max(INITIAL_FILE_CAPACITY);
            slot_entry.data_handle = rt::memory_create(capacity, true)?;
            slot_entry.data_capacity = capacity;
            slot_entry.data_len = data_len;
            load_file_data(store, slot, file_offset, data_len, slot_entry.data_handle)?;
        }
    }

    // Mount records trail the entry records inside the metadata region.
    let mounts_offset = records_offset + entry_count * PERSISTENT_RECORD_BYTES;
    let mut stored_mounts = 0usize;
    if mount_count > 0 {
        // Merge persisted mounts over the seeded defaults: user mounts layer
        // on top, built-in roots always stay available.
        for record_index in 0..mount_count {
            let record_offset = mounts_offset + record_index * MOUNT_RECORD_BYTES;
            let record = read_record_span(store, slot, record_offset)?;
            if record[0] == 0 {
                continue;
            }
            let kind = match u32::from_le_bytes(record[4..8].try_into().unwrap()) {
                x if x == StorageMountKind::Boot as u32 => StorageMountKind::Boot,
                x if x == StorageMountKind::Persistent as u32 => StorageMountKind::Persistent,
                x if x == StorageMountKind::Ephemeral as u32 => StorageMountKind::Ephemeral,
                x if x == StorageMountKind::Temp as u32 => StorageMountKind::Temp,
                _ => continue,
            };
            let path_len = u16::from_le_bytes(record[2..4].try_into().unwrap()) as usize;
            let flags = u64::from_le_bytes(record[8..16].try_into().unwrap());
            let authority = u64::from_le_bytes(record[16..24].try_into().unwrap());
            if path_len > rt::STORAGE_MOUNT_PATH_MAX {
                continue;
            }
            let target = match rt::storage_find_mount_by_path(mounts, &record[24..24 + path_len]) {
                Some(index) => &mut mounts[index],
                None => match mounts.iter_mut().find(|mount| !mount.occupied) {
                    Some(slot_mount) => slot_mount,
                    None => break,
                },
            };
            let _ = target.install(&record[24..24 + path_len], kind, flags, authority);
            stored_mounts += 1;
        }
    }

    Ok(Some((generation, stored_mounts)))
}

/// Reads one metadata record that may straddle a block boundary.
pub(crate) fn read_record_span(
    store: &PersistentStore,
    slot: usize,
    record_offset: usize,
) -> rt::Result<[u8; PERSISTENT_RECORD_BYTES]> {
    let block_size = store.block_size;
    let mut out = [0u8; PERSISTENT_RECORD_BYTES];
    let mut copied = 0usize;
    while copied < PERSISTENT_RECORD_BYTES {
        let absolute = record_offset + copied;
        let block_index = absolute / block_size;
        let block_offset = absolute % block_size;
        let copy_len = (PERSISTENT_RECORD_BYTES - copied).min(block_size - block_offset);
        let mut block = [0u8; BLOCK_BUFFER_BYTES];
        rt::block_device_read(
            store.handle,
            (slot * store.slot_blocks + block_index) as u64,
            &mut block[..block_size],
        )?;
        out[copied..copied + copy_len]
            .copy_from_slice(&block[block_offset..block_offset + copy_len]);
        copied += copy_len;
    }
    Ok(out)
}

fn load_file_data(
    store: &PersistentStore,
    slot: usize,
    offset: usize,
    len: usize,
    destination: Handle,
) -> rt::Result<()> {
    if len == 0 {
        return Ok(());
    }
    let block_size = store.block_size;
    let mut block = [0u8; BLOCK_BUFFER_BYTES];
    let mut copied = 0usize;
    while copied < len {
        let absolute = offset + copied;
        let block_index = absolute / block_size;
        let block_offset = absolute % block_size;
        let copy_len = (len - copied).min(block_size - block_offset);
        rt::block_device_read(
            store.handle,
            (slot * store.slot_blocks + block_index) as u64,
            &mut block[..block_size],
        )?;
        let _ = rt::memory_write(
            destination,
            copied,
            &block[block_offset..block_offset + copy_len],
        )?;
        copied += copy_len;
    }
    Ok(())
}

fn flush_persistent_store(
    store: &mut PersistentStore,
    mounts: &MountTable,
    mutable_entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
) -> rt::Result<()> {
    let block_size = store.block_size;
    let mut header_block = [0u8; BLOCK_BUFFER_BYTES];
    let mut scratch_block = [0u8; BLOCK_BUFFER_BYTES];
    if block_size > header_block.len() {
        return Err(rt::Error::BufferTooSmall);
    }

    let next_slot = (store.active_slot + 1) % 2;
    let seed_entry: &MutableEntry = &mutable_entries[0];
    let mut records: [&MutableEntry; MAX_MUTABLE_ENTRIES] = [seed_entry; MAX_MUTABLE_ENTRIES];
    let mut record_count = 0usize;
    for entry in mutable_entries
        .iter()
        .filter(|entry| entry.occupied && entry.persistent)
    {
        records[record_count] = entry;
        record_count += 1;
    }
    let mut active_mounts: [&StorageMount; STORAGE_MOUNT_TABLE_MAX] =
        [&mounts[0]; STORAGE_MOUNT_TABLE_MAX];
    let mut mount_count = 0usize;
    for mount in mounts.iter().filter(|mount| mount.occupied) {
        active_mounts[mount_count] = mount;
        mount_count += 1;
    }
    let records_offset = block_size;
    let mounts_offset = records_offset + record_count * PERSISTENT_RECORD_BYTES;
    let mut data_cursor = align_up(mounts_offset + mount_count * MOUNT_RECORD_BYTES, block_size);
    let data_offset = data_cursor;
    for entry in &records {
        if entry.kind == rt::StorageEntryKind::File {
            data_cursor = align_up(data_cursor, block_size);
            data_cursor = data_cursor.saturating_add(entry.data_len);
        }
    }
    let total_bytes = align_up(data_cursor, block_size);
    if total_bytes > store.slot_blocks * block_size {
        return Err(rt::Error::Busy);
    }

    let generation = store.generation.saturating_add(1);
    header_block[..8].copy_from_slice(&PERSISTENT_MAGIC);
    header_block[8..12].copy_from_slice(&PERSISTENT_VERSION.to_le_bytes());
    header_block[12..16].copy_from_slice(&(record_count as u32).to_le_bytes());
    header_block[16..24].copy_from_slice(&generation.to_le_bytes());
    header_block[24..32].copy_from_slice(&(records_offset as u64).to_le_bytes());
    header_block[32..40].copy_from_slice(&(data_offset as u64).to_le_bytes());
    header_block[40..48].copy_from_slice(&(total_bytes as u64).to_le_bytes());
    header_block[48..52].copy_from_slice(&(mount_count as u32).to_le_bytes());

    // Plan per-file data offsets before emitting any records.
    let mut file_offsets = [0usize; MAX_MUTABLE_ENTRIES];
    let mut file_plan_cursor = data_offset;
    for index in 0..record_count {
        let entry = records[index];
        if entry.kind == rt::StorageEntryKind::File {
            file_plan_cursor = align_up(file_plan_cursor, block_size);
            file_offsets[index] = file_plan_cursor;
            file_plan_cursor += entry.data_len;
        }
    }

    let build_entry_record = |index: usize| -> [u8; PERSISTENT_RECORD_BYTES] {
        let entry = records[index];
        let mut record = [0u8; PERSISTENT_RECORD_BYTES];
        record[0] = 1;
        record[1] = entry.kind as u32 as u8;
        record[2..4].copy_from_slice(&(entry.path_len as u16).to_le_bytes());
        record[8..16].copy_from_slice(&(entry.data_len as u64).to_le_bytes());
        if entry.kind == rt::StorageEntryKind::File {
            record[16..24].copy_from_slice(&(file_offsets[index] as u64).to_le_bytes());
        }
        record[24..24 + entry.path_len].copy_from_slice(&entry.path[..entry.path_len]);
        record
    };
    let build_mount_record = |index: usize| -> [u8; PERSISTENT_RECORD_BYTES] {
        let mount = active_mounts[index];
        debug_assert_eq!(MOUNT_RECORD_BYTES, PERSISTENT_RECORD_BYTES);
        let mut record = [0u8; PERSISTENT_RECORD_BYTES];
        record[0] = 1;
        record[2..4].copy_from_slice(&(mount.path_len as u16).to_le_bytes());
        record[4..8].copy_from_slice(&(mount.kind as u32).to_le_bytes());
        record[8..16].copy_from_slice(&mount.flags.to_le_bytes());
        record[16..24].copy_from_slice(&mount.authority.to_le_bytes());
        record[24..24 + mount.path_len].copy_from_slice(&mount.path[..mount.path_len]);
        record
    };

    // Metadata integrity: FNV-1a64 over the checksum-free header prefix and
    // every metadata record, stored at offset 52. Zero means absent so older
    // snapshots keep loading untouched.
    let mut summer = crate::fsck::SnapshotChecksummer::new();
    summer.feed(&header_block[..52]);
    for index in 0..record_count {
        summer.feed(&build_entry_record(index));
    }
    for index in 0..mount_count {
        summer.feed(&build_mount_record(index));
    }
    header_block[52..60].copy_from_slice(&summer.finish().to_le_bytes());

    rt::block_device_write(
        store.handle,
        (next_slot * store.slot_blocks) as u64,
        &header_block[..block_size],
    )?;

    write_record_batch(
        store,
        next_slot,
        records_offset,
        &mut scratch_block,
        record_count,
        |index| build_entry_record(index),
    )?;

    // File payloads land after all metadata so partial writes stay parseable.
    for index in 0..record_count {
        let entry = records[index];
        if entry.kind == rt::StorageEntryKind::File && file_offsets[index] != 0 {
            flush_file_data(store, next_slot, file_offsets[index], entry)?;
        }
    }

    write_record_batch(
        store,
        next_slot,
        mounts_offset,
        &mut scratch_block,
        mount_count,
        |index| build_mount_record(index),
    )?;

    store.active_slot = next_slot;
    store.generation = generation;
    Ok(())
}

/// Writes fixed-size records that may span block boundaries.
fn write_record_batch<const N: usize>(
    store: &PersistentStore,
    slot: usize,
    base_offset: usize,
    scratch: &mut [u8; BLOCK_BUFFER_BYTES],
    count: usize,
    build: impl Fn(usize) -> [u8; N],
) -> rt::Result<()> {
    let block_size = store.block_size;
    for index in 0..count {
        let record = build(index);
        let record_offset = base_offset + index * N;
        let mut written = 0usize;
        while written < N {
            let absolute = record_offset + written;
            let block_index = absolute / block_size;
            let block_offset = absolute % block_size;
            let copy_len = (N - written).min(block_size - block_offset);
            scratch[block_offset..block_offset + copy_len]
                .copy_from_slice(&record[written..written + copy_len]);
            let end_of_record_in_block =
                block_offset + copy_len == block_size || written + copy_len == N;
            if end_of_record_in_block {
                rt::block_device_write(
                    store.handle,
                    (slot * store.slot_blocks + block_index) as u64,
                    &scratch[..block_size],
                )?;
                // Keep untouched tail zeroed for subsequent spans.
                scratch[block_offset..block_size].fill(0);
            }
            written += copy_len;
        }
    }
    Ok(())
}

fn flush_file_data(
    store: &PersistentStore,
    slot: usize,
    offset: usize,
    entry: &MutableEntry,
) -> rt::Result<()> {
    if entry.data_len == 0 {
        return Ok(());
    }
    let block_size = store.block_size;
    let mut block = [0u8; BLOCK_BUFFER_BYTES];
    let mut copied = 0usize;
    while copied < entry.data_len {
        block[..block_size].fill(0);
        let copy_len = (entry.data_len - copied).min(block_size);
        let read = rt::memory_read(entry.data_handle, copied, &mut block[..copy_len])?;
        rt::block_device_write(
            store.handle,
            (slot * store.slot_blocks + (offset + copied) / block_size) as u64,
            &block[..block_size],
        )?;
        copied += read;
    }
    Ok(())
}

fn align_up(value: usize, align: usize) -> usize {
    if value.is_multiple_of(align) {
        value
    } else {
        value + (align - (value % align))
    }
}

pub(crate) fn release_mutable_entry(entry: &mut MutableEntry) {
    if entry.data_handle != rt::INVALID_HANDLE {
        let _ = rt::handle_close(entry.data_handle);
    }
    *entry = MutableEntry::empty();
}

pub(crate) fn release_blob_session(session: &mut crate::BlobSession) {
    if session.endpoint != rt::INVALID_HANDLE {
        let _ = rt::handle_close(session.endpoint);
    }
    *session = crate::BlobSession::empty();
}

pub(crate) fn release_directory_session(session: &mut crate::DirectorySession) {
    if session.endpoint != rt::INVALID_HANDLE {
        let _ = rt::handle_close(session.endpoint);
    }
    *session = crate::DirectorySession::empty();
}
