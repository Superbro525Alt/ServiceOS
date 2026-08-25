use rt::{Handle, IPC_MAX_WORDS, RawMessage, StorageEntryKind, StorageStatus, StorageTag};
use serviceos_userspace_runtime as rt;

use crate::{
    BlobSession, BlobSource, INITIAL_FILE_CAPACITY, MAX_MUTABLE_ENTRIES, MountTable, MutableEntry,
    PersistentStore,
    index::{ORIGIN_MUTABLE, SearchIndex},
    persistent::persist_state,
    util::{pack_bytes, unpack_bytes},
};

pub(crate) fn handle_blob_request(
    bootstore_handle: Handle,
    mounts: &MountTable,
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
    persistent_store: Option<&mut PersistentStore>,
    search_index: &mut SearchIndex,
    session: &mut BlobSession,
    message: &RawMessage,
) -> rt::Result<()> {
    let _ = mounts;
    if message.tag == StorageTag::CloseRequest as u32 {
        crate::release_blob_session(session);
        return Ok(());
    }

    match message.tag {
        x if x == StorageTag::ReadRequest as u32 => {
            handle_read_request(bootstore_handle, mutable_entries, session, message)
        }
        x if x == StorageTag::WriteRequest as u32 => handle_write_request(
            mounts,
            mutable_entries,
            persistent_store,
            search_index,
            session,
            message,
        ),
        _ => Ok(()),
    }
}

fn handle_read_request(
    bootstore_handle: Handle,
    mutable_entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
    session: &mut BlobSession,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 2 || message.handle_count < 1 {
        return Ok(());
    }

    let reply_handle = message.handles[0];
    let offset = message.words[0] as usize;
    let requested = message.words[1] as usize;
    let mut reply = RawMessage::empty(StorageTag::ReadReply as u32);
    reply.word_count = 3;
    reply.words[1] = offset as u64;

    if offset > session.data_len {
        reply.words[0] = StorageStatus::InvalidOffset as u32 as u64;
        let _ = rt::channel_send(reply_handle, &reply);
        let _ = rt::handle_close(reply_handle);
        return Ok(());
    }

    let available = session.data_len - offset;
    let payload_capacity = (IPC_MAX_WORDS - 3) * 8;
    let read_len = available.min(requested).min(payload_capacity);
    let mut bytes = [0u8; (IPC_MAX_WORDS - 3) * 8];
    let copied = match session.source {
        BlobSource::BootStore => rt::memory_read(
            bootstore_handle,
            session.data_offset + offset,
            &mut bytes[..read_len],
        )?,
        BlobSource::Mutable => {
            let Some(entry) = mutable_entries
                .get(session.entry_index)
                .filter(|entry| entry.occupied)
            else {
                reply.words[0] = StorageStatus::NotFound as u32 as u64;
                let _ = rt::channel_send(reply_handle, &reply);
                let _ = rt::handle_close(reply_handle);
                return Ok(());
            };
            rt::memory_read(entry.data_handle, offset, &mut bytes[..read_len])?
        }
    };

    reply.words[0] = StorageStatus::Ok as u32 as u64;
    reply.words[2] = copied as u64;
    reply.word_count += pack_bytes(&bytes[..copied], &mut reply.words[3..])?;
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

fn handle_write_request(
    mounts: &MountTable,
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
    persistent_store: Option<&mut PersistentStore>,
    search_index: &mut SearchIndex,
    session: &mut BlobSession,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 3 || message.handle_count < 1 {
        return Ok(());
    }

    let reply_handle = message.handles[0];
    let mut reply = RawMessage::empty(StorageTag::WriteReply as u32);
    reply.word_count = 2;
    reply.words[1] = session.data_len as u64;

    if !session.writable || session.source != BlobSource::Mutable {
        reply.words[0] = StorageStatus::Denied as u32 as u64;
        let _ = rt::channel_send(reply_handle, &reply);
        let _ = rt::handle_close(reply_handle);
        return Ok(());
    }

    let offset = message.words[0] as usize;
    let total_len = message.words[1] as usize;
    let write_len = message.words[2] as usize;
    if total_len < offset.saturating_add(write_len) {
        reply.words[0] = StorageStatus::InvalidOffset as u32 as u64;
        let _ = rt::channel_send(reply_handle, &reply);
        let _ = rt::handle_close(reply_handle);
        return Ok(());
    }
    let Some(entry) = mutable_entries
        .get_mut(session.entry_index)
        .filter(|entry| entry.occupied)
    else {
        reply.words[0] = StorageStatus::NotFound as u32 as u64;
        let _ = rt::channel_send(reply_handle, &reply);
        let _ = rt::handle_close(reply_handle);
        return Ok(());
    };

    ensure_mutable_capacity(entry, total_len)?;
    let mut bytes = [0u8; (IPC_MAX_WORDS - 3) * 8];
    unpack_bytes(
        &message.words[3..message.word_count as usize],
        write_len,
        &mut bytes,
    )?;
    let _ = rt::memory_write(entry.data_handle, offset, &bytes[..write_len])?;
    entry.data_len = total_len;
    session.data_len = total_len;
    search_index.upsert(
        &entry.path[..entry.path_len],
        StorageEntryKind::File,
        total_len as u64,
        rt::monotonic_now().unwrap_or(0),
        ORIGIN_MUTABLE,
    );
    persist_state(persistent_store, mounts, mutable_entries)?;
    reply.words[0] = StorageStatus::Ok as u32 as u64;
    reply.words[1] = total_len as u64;
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

fn ensure_mutable_capacity(entry: &mut MutableEntry, total_len: usize) -> rt::Result<()> {
    if total_len <= entry.data_capacity {
        return Ok(());
    }
    let mut new_capacity = entry.data_capacity.max(INITIAL_FILE_CAPACITY);
    while new_capacity < total_len {
        new_capacity = new_capacity.saturating_mul(2);
    }
    let new_handle = rt::memory_create(new_capacity, true)?;
    if entry.data_len > 0 {
        let mut copied = 0usize;
        let mut buffer = [0u8; 128];
        while copied < entry.data_len {
            let remaining = entry.data_len - copied;
            let chunk_len = remaining.min(buffer.len());
            let read = rt::memory_read(entry.data_handle, copied, &mut buffer[..chunk_len])?;
            if read == 0 {
                break;
            }
            let _ = rt::memory_write(new_handle, copied, &buffer[..read])?;
            copied += read;
        }
    }
    if entry.data_handle != rt::INVALID_HANDLE {
        let _ = rt::handle_close(entry.data_handle);
    }
    entry.data_handle = new_handle;
    entry.data_capacity = new_capacity;
    Ok(())
}
