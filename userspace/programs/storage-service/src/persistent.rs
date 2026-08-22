use serviceos_userspace_runtime as rt;
use rt::{BlockDeviceBackend, Handle};

use crate::{
    state::{
        MutableEntry, PersistentStore, MAX_MUTABLE_ENTRIES, BLOCK_BUFFER_BYTES,
        INITIAL_FILE_CAPACITY, PERSISTENT_MAGIC, PERSISTENT_RECORD_BYTES, PERSISTENT_VERSION,
    },
};

pub(crate) fn initialize_persistent_store(
    block_handle: Handle,
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
        if let Some(generation) = load_persistent_slot(&best_store, slot, mutable_entries)? {
            if !loaded_any || generation >= best_generation {
                best_generation = generation;
                best_store.active_slot = slot;
                best_store.generation = generation;
                loaded_any = true;
            }
        }
    }

    if loaded_any {
        load_persistent_slot(&best_store, best_store.active_slot, mutable_entries)?;
        let _ = rt::write_logf(
            "storage",
            format_args!(
                "mounted persistent snapshot blocks={} generation={}",
                slot_blocks, best_generation
            ),
        );
    } else {
        let _ = rt::write_logf(
            "storage",
            format_args!("initialized empty persistent snapshot blocks={}", slot_blocks),
        );
    }

    Ok(Some(best_store))
}

pub(crate) fn persist_mutable_entries(
    persistent_store: Option<&mut PersistentStore>,
    mutable_entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
) -> rt::Result<()> {
    let Some(store) = persistent_store else {
        return Ok(());
    };
    flush_persistent_store(store, mutable_entries)
}

pub(crate) fn load_persistent_slot(
    store: &PersistentStore,
    slot: usize,
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
) -> rt::Result<Option<u64>> {
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
    if block[..PERSISTENT_MAGIC.len()] != PERSISTENT_MAGIC {
        return Ok(None);
    }
    let version = u32::from_le_bytes(block[8..12].try_into().unwrap());
    if version != PERSISTENT_VERSION {
        return Ok(None);
    }
    let generation = u64::from_le_bytes(block[16..24].try_into().unwrap());
    let entry_count = u32::from_le_bytes(block[12..16].try_into().unwrap()) as usize;
    let records_offset = u64::from_le_bytes(block[24..32].try_into().unwrap()) as usize;
    let data_offset = u64::from_le_bytes(block[32..40].try_into().unwrap()) as usize;
    let total_bytes = u64::from_le_bytes(block[40..48].try_into().unwrap()) as usize;
    if entry_count > MAX_MUTABLE_ENTRIES
        || records_offset < block_size
        || total_bytes == 0
        || total_bytes > store.slot_blocks * block_size
    {
        return Ok(None);
    }
    let minimum_data_offset =
        align_up(records_offset + entry_count * PERSISTENT_RECORD_BYTES, block_size);
    if data_offset < minimum_data_offset {
        return Ok(None);
    }

    for entry in mutable_entries.iter_mut() {
        release_mutable_entry(entry);
    }

    for record_index in 0..entry_count {
        let record_offset = records_offset + record_index * PERSISTENT_RECORD_BYTES;
        let block_index = record_offset / block_size;
        let block_offset = record_offset % block_size;
        if block_offset + PERSISTENT_RECORD_BYTES > block_size {
            return Ok(None);
        }
        rt::block_device_read(
            store.handle,
            (slot * store.slot_blocks + block_index) as u64,
            &mut block[..block_size],
        )?;
        let record = &block[block_offset..block_offset + PERSISTENT_RECORD_BYTES];
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
        slot_entry.occupied = true;
        if kind == rt::StorageEntryKind::File {
            let capacity = data_len.max(INITIAL_FILE_CAPACITY);
            slot_entry.data_handle = rt::memory_create(capacity, true)?;
            slot_entry.data_capacity = capacity;
            slot_entry.data_len = data_len;
            load_file_data(store, slot, file_offset, data_len, slot_entry.data_handle)?;
        }
    }

    Ok(Some(generation))
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
        let _ = rt::memory_write(destination, copied, &block[block_offset..block_offset + copy_len])?;
        copied += copy_len;
    }
    Ok(())
}

fn flush_persistent_store(
    store: &mut PersistentStore,
    mutable_entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
) -> rt::Result<()> {
    let block_size = store.block_size;
    let mut header_block = [0u8; BLOCK_BUFFER_BYTES];
    let mut scratch_block = [0u8; BLOCK_BUFFER_BYTES];
    if block_size > header_block.len() {
        return Err(rt::Error::BufferTooSmall);
    }

    let next_slot = (store.active_slot + 1) % 2;
    let mut records = 0usize;
    let records_offset = block_size;
    for _entry in mutable_entries
        .iter()
        .filter(|entry| entry.occupied && is_persistent_path(&entry.path[..entry.path_len]))
    {
        records += 1;
    }
    let mut data_cursor =
        align_up(records_offset + records * PERSISTENT_RECORD_BYTES, block_size);
    let data_offset = data_cursor;
    for entry in mutable_entries
        .iter()
        .filter(|entry| entry.occupied && is_persistent_path(&entry.path[..entry.path_len]))
    {
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
    header_block[12..16].copy_from_slice(&(records as u32).to_le_bytes());
    header_block[16..24].copy_from_slice(&generation.to_le_bytes());
    header_block[24..32].copy_from_slice(&(records_offset as u64).to_le_bytes());
    header_block[32..40].copy_from_slice(&(data_offset as u64).to_le_bytes());
    header_block[40..48].copy_from_slice(&(total_bytes as u64).to_le_bytes());

    rt::block_device_write(
        store.handle,
        (next_slot * store.slot_blocks) as u64,
        &header_block[..block_size],
    )?;

    let mut record_cursor = 0usize;
    let mut file_cursor = data_offset;
    for entry in mutable_entries
        .iter()
        .filter(|entry| entry.occupied && is_persistent_path(&entry.path[..entry.path_len]))
    {
        let record_offset = records_offset + record_cursor * PERSISTENT_RECORD_BYTES;
        let block_index = record_offset / block_size;
        let block_offset = record_offset % block_size;
        if block_offset == 0 {
            scratch_block[..block_size].fill(0);
        }
        let record = &mut scratch_block[block_offset..block_offset + PERSISTENT_RECORD_BYTES];
        record.fill(0);
        record[0] = 1;
        record[1] = entry.kind as u32 as u8;
        record[2..4].copy_from_slice(&(entry.path_len as u16).to_le_bytes());
        record[8..16].copy_from_slice(&(entry.data_len as u64).to_le_bytes());
        if entry.kind == rt::StorageEntryKind::File {
            file_cursor = align_up(file_cursor, block_size);
            record[16..24].copy_from_slice(&(file_cursor as u64).to_le_bytes());
        }
        record[24..24 + entry.path_len].copy_from_slice(&entry.path[..entry.path_len]);
        let end_of_block = block_offset + PERSISTENT_RECORD_BYTES == block_size || record_cursor + 1 == records;
        if end_of_block {
            rt::block_device_write(
                store.handle,
                (next_slot * store.slot_blocks + block_index) as u64,
                &scratch_block[..block_size],
            )?;
        }
        if entry.kind == rt::StorageEntryKind::File {
            flush_file_data(store, next_slot, file_cursor, entry)?;
            file_cursor += entry.data_len;
        }
        record_cursor += 1;
    }

    store.active_slot = next_slot;
    store.generation = generation;
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
    if value % align == 0 {
        value
    } else {
        value + (align - (value % align))
    }
}

pub(crate) fn is_persistent_path(path: &[u8]) -> bool {
    crate::PERSISTENT_ROOTS
        .iter()
        .any(|root| path.len() >= root.len() && path[..root.len()] == **root)
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
