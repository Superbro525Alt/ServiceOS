use serviceos_bundle::BootStoreEntryKind;
use serviceos_userspace_runtime as rt;
use rt::{RawMessage, StorageEntryKind, StorageStatus, StorageTag};

use crate::{
    path::{
        boot_directory_exists, compose_child_path, directory_child_from_path, find_mutable_entry,
        is_mutable_path, mutable_directory_has_children, mutable_root_has_materialized_children,
    },
    persistent::persist_mutable_entries,
    util::{pack_bytes, send_blob_open_reply, send_status_only, unpack_bytes},
    BlobSession, DirectorySession, EntrySlot, MutableEntry, PersistentStore, MAX_BLOB_SESSIONS,
    MAX_MUTABLE_ENTRIES, MAX_STORAGE_PATH, MUTABLE_ROOTS, INITIAL_FILE_CAPACITY,
};

pub(crate) fn handle_directory_request(
    entries: &[EntrySlot],
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
    blob_sessions: &mut [BlobSession; MAX_BLOB_SESSIONS],
    persistent_store: Option<&mut PersistentStore>,
    session: &mut DirectorySession,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.tag == StorageTag::CloseRequest as u32 {
        crate::release_directory_session(session);
        return Ok(());
    }

    match message.tag {
        x if x == StorageTag::DirectoryReadRequest as u32 => {
            handle_directory_read_request(entries, mutable_entries, session, message)
        }
        x if x == StorageTag::DirectoryCreateRequest as u32 => {
            handle_directory_create_request(mutable_entries, persistent_store, session, message)
        }
        x if x == StorageTag::DirectoryRemoveRequest as u32 => {
            handle_directory_remove_request(
                entries,
                mutable_entries,
                persistent_store,
                session,
                message,
            )
        }
        x if x == StorageTag::DirectoryOpenFileRequest as u32 => {
            handle_directory_open_file_request(
                entries,
                mutable_entries,
                blob_sessions,
                persistent_store,
                session,
                message,
            )
        }
        _ => Ok(()),
    }
}

fn handle_directory_read_request(
    entries: &[EntrySlot],
    mutable_entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
    session: &DirectorySession,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 1 || message.handle_count < 1 {
        return Ok(());
    }

    let cursor = message.words[0] as usize;
    let prefix = &session.path[..session.path_len];
    let reply_handle = message.handles[0];

    let mut reply = RawMessage::empty(StorageTag::DirectoryReadReply as u32);
    reply.word_count = 4;
    reply.words[0] = StorageStatus::End as u32 as u64;
    reply.words[1] = cursor as u64;
    reply.words[2] = StorageEntryKind::File as u32 as u64;
    reply.words[3] = 0;

    let mut seen = 0usize;
    if prefix.is_empty() {
        for root in MUTABLE_ROOTS {
            if mutable_root_has_materialized_children(entries, mutable_entries, root) {
                continue;
            }
            if seen == cursor {
                reply.words[0] = StorageStatus::Ok as u32 as u64;
                reply.words[1] = (seen + 1) as u64;
                reply.words[2] = StorageEntryKind::Directory as u32 as u64;
                reply.words[3] = root.len() as u64;
                reply.word_count += pack_bytes(root, &mut reply.words[4..])?;
                let _ = rt::channel_send(reply_handle, &reply);
                let _ = rt::handle_close(reply_handle);
                return Ok(());
            }
            seen += 1;
        }
    }

    for entry in entries {
        if let Some((child_path, child_kind)) =
            directory_child_from_path(&entry.path[..entry.path_len], prefix)
        {
            if seen == cursor {
                reply.words[0] = StorageStatus::Ok as u32 as u64;
                reply.words[1] = (seen + 1) as u64;
                reply.words[2] = child_kind as u32 as u64;
                reply.words[3] = child_path.len() as u64;
                reply.word_count += pack_bytes(child_path, &mut reply.words[4..])?;
                let _ = rt::channel_send(reply_handle, &reply);
                let _ = rt::handle_close(reply_handle);
                return Ok(());
            }
            seen += 1;
        }
    }

    for entry in mutable_entries.iter().filter(|entry| entry.occupied) {
        if let Some((child_path, child_kind)) =
            directory_child_from_path(&entry.path[..entry.path_len], prefix)
        {
            if seen == cursor {
                reply.words[0] = StorageStatus::Ok as u32 as u64;
                reply.words[1] = (seen + 1) as u64;
                reply.words[2] = child_kind as u32 as u64;
                reply.words[3] = child_path.len() as u64;
                reply.word_count += pack_bytes(child_path, &mut reply.words[4..])?;
                let _ = rt::channel_send(reply_handle, &reply);
                let _ = rt::handle_close(reply_handle);
                return Ok(());
            }
            seen += 1;
        }
    }

    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

fn handle_directory_create_request(
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
    persistent_store: Option<&mut PersistentStore>,
    session: &DirectorySession,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 2 || message.handle_count < 1 {
        return Ok(());
    }

    let reply_handle = message.handles[0];
    if !session.writable {
        send_status_only(reply_handle, StorageTag::DirectoryCreateReply, StorageStatus::Denied);
        return Ok(());
    }

    let kind = match message.words[0] as u32 {
        x if x == StorageEntryKind::File as u32 => StorageEntryKind::File,
        x if x == StorageEntryKind::Directory as u32 => StorageEntryKind::Directory,
        _ => {
            send_status_only(reply_handle, StorageTag::DirectoryCreateReply, StorageStatus::InvalidPath);
            return Ok(());
        }
    };
    let name_len = message.words[1] as usize;
    let mut name = [0u8; MAX_STORAGE_PATH];
    if unpack_bytes(&message.words[2..message.word_count as usize], name_len, &mut name).is_err() {
        send_status_only(reply_handle, StorageTag::DirectoryCreateReply, StorageStatus::InvalidPath);
        return Ok(());
    }
    let Some((path, path_len)) = compose_child_path(&session.path[..session.path_len], &name[..name_len], kind) else {
        send_status_only(reply_handle, StorageTag::DirectoryCreateReply, StorageStatus::InvalidPath);
        return Ok(());
    };
    if !is_mutable_path(&path[..path_len]) {
        send_status_only(reply_handle, StorageTag::DirectoryCreateReply, StorageStatus::Denied);
        return Ok(());
    }
    if find_mutable_entry(mutable_entries, &path[..path_len]).is_some() {
        send_status_only(
            reply_handle,
            StorageTag::DirectoryCreateReply,
            StorageStatus::AlreadyExists,
        );
        return Ok(());
    }
    let Some(slot) = mutable_entries.iter_mut().find(|entry| !entry.occupied) else {
        send_status_only(reply_handle, StorageTag::DirectoryCreateReply, StorageStatus::Busy);
        return Ok(());
    };
    *slot = MutableEntry::empty();
    slot.kind = kind;
    slot.path[..path_len].copy_from_slice(&path[..path_len]);
    slot.path_len = path_len;
    slot.occupied = true;
    if kind == StorageEntryKind::File {
        slot.data_handle = rt::memory_create(INITIAL_FILE_CAPACITY, true)?;
        slot.data_capacity = INITIAL_FILE_CAPACITY;
        slot.data_len = 0;
    }
    persist_mutable_entries(persistent_store, mutable_entries)?;
    send_status_only(reply_handle, StorageTag::DirectoryCreateReply, StorageStatus::Ok);
    Ok(())
}

fn handle_directory_remove_request(
    entries: &[EntrySlot],
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
    persistent_store: Option<&mut PersistentStore>,
    session: &DirectorySession,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 1 || message.handle_count < 1 {
        return Ok(());
    }

    let reply_handle = message.handles[0];
    if !session.writable {
        send_status_only(reply_handle, StorageTag::DirectoryRemoveReply, StorageStatus::Denied);
        return Ok(());
    }

    let name_len = message.words[0] as usize;
    let mut name = [0u8; MAX_STORAGE_PATH];
    if unpack_bytes(&message.words[1..message.word_count as usize], name_len, &mut name).is_err() {
        send_status_only(reply_handle, StorageTag::DirectoryRemoveReply, StorageStatus::InvalidPath);
        return Ok(());
    }

    let file_path = compose_child_path(
        &session.path[..session.path_len],
        &name[..name_len],
        StorageEntryKind::File,
    );
    let dir_path = compose_child_path(
        &session.path[..session.path_len],
        &name[..name_len],
        StorageEntryKind::Directory,
    );

    if let Some((path, path_len)) = file_path {
        if let Some(index) = find_mutable_entry(mutable_entries, &path[..path_len]) {
            crate::release_mutable_entry(&mut mutable_entries[index]);
            persist_mutable_entries(persistent_store, mutable_entries)?;
            send_status_only(reply_handle, StorageTag::DirectoryRemoveReply, StorageStatus::Ok);
            return Ok(());
        }
        if entries.iter().any(|entry| entry.matches(&path[..path_len])) {
            send_status_only(reply_handle, StorageTag::DirectoryRemoveReply, StorageStatus::Denied);
            return Ok(());
        }
    }
    if let Some((path, path_len)) = dir_path {
        if let Some(index) = find_mutable_entry(mutable_entries, &path[..path_len]) {
            if mutable_directory_has_children(mutable_entries, &path[..path_len]) {
                send_status_only(reply_handle, StorageTag::DirectoryRemoveReply, StorageStatus::Busy);
                return Ok(());
            }
            crate::release_mutable_entry(&mut mutable_entries[index]);
            persist_mutable_entries(persistent_store, mutable_entries)?;
            send_status_only(reply_handle, StorageTag::DirectoryRemoveReply, StorageStatus::Ok);
            return Ok(());
        }
        if boot_directory_exists(entries, &path[..path_len]) {
            send_status_only(reply_handle, StorageTag::DirectoryRemoveReply, StorageStatus::Denied);
            return Ok(());
        }
    }

    send_status_only(reply_handle, StorageTag::DirectoryRemoveReply, StorageStatus::NotFound);
    Ok(())
}

fn handle_directory_open_file_request(
    entries: &[EntrySlot],
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
    blob_sessions: &mut [crate::BlobSession; crate::MAX_BLOB_SESSIONS],
    persistent_store: Option<&mut PersistentStore>,
    session: &DirectorySession,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 3 || message.handle_count < 1 {
        return Ok(());
    }

    let reply_handle = message.handles[0];
    let name_len = message.words[0] as usize;
    let create = message.words[1] != 0;
    let writable = message.words[2] != 0;
    let mut name = [0u8; MAX_STORAGE_PATH];
    if unpack_bytes(&message.words[3..message.word_count as usize], name_len, &mut name).is_err() {
        send_blob_open_reply(
            StorageTag::DirectoryOpenFileReply,
            reply_handle,
            StorageStatus::InvalidPath,
            0,
            None,
        );
        return Ok(());
    }
    let Some((path, path_len)) = compose_child_path(
        &session.path[..session.path_len],
        &name[..name_len],
        StorageEntryKind::File,
    ) else {
        send_blob_open_reply(
            StorageTag::DirectoryOpenFileReply,
            reply_handle,
            StorageStatus::InvalidPath,
            0,
            None,
        );
        return Ok(());
    };

    if (create || writable) && !session.writable {
        send_blob_open_reply(
            StorageTag::DirectoryOpenFileReply,
            reply_handle,
            StorageStatus::Denied,
            0,
            None,
        );
        return Ok(());
    }

    let Some(blob_session) = blob_sessions.iter_mut().find(|entry| !entry.occupied) else {
        send_blob_open_reply(
            StorageTag::DirectoryOpenFileReply,
            reply_handle,
            StorageStatus::Busy,
            0,
            None,
        );
        return Ok(());
    };

    if let Some(index) = find_mutable_entry(mutable_entries, &path[..path_len]) {
        if mutable_entries[index].kind != StorageEntryKind::File {
            send_blob_open_reply(
                StorageTag::DirectoryOpenFileReply,
                reply_handle,
                StorageStatus::InvalidPath,
                0,
                None,
            );
            return Ok(());
        }
        let pair = rt::channel_create()?;
        blob_session.endpoint = pair.first;
        blob_session.source = crate::BlobSource::Mutable;
        blob_session.data_offset = 0;
        blob_session.data_len = mutable_entries[index].data_len;
        blob_session.data_handle = mutable_entries[index].data_handle;
        blob_session.entry_index = index;
        blob_session.writable = writable;
        blob_session.occupied = true;
        send_blob_open_reply(
            StorageTag::DirectoryOpenFileReply,
            reply_handle,
            StorageStatus::Ok,
            mutable_entries[index].data_len,
            Some(pair.second),
        );
        let _ = rt::handle_close(pair.second);
        return Ok(());
    }

    if writable || create {
        if !is_mutable_path(&path[..path_len]) {
            send_blob_open_reply(
                StorageTag::DirectoryOpenFileReply,
                reply_handle,
                StorageStatus::Denied,
                0,
                None,
            );
            return Ok(());
        }
        let Some(slot_index) = mutable_entries.iter().position(|entry| !entry.occupied) else {
            send_blob_open_reply(
                StorageTag::DirectoryOpenFileReply,
                reply_handle,
                StorageStatus::Busy,
                0,
                None,
            );
            return Ok(());
        };
        mutable_entries[slot_index] = MutableEntry::empty();
        mutable_entries[slot_index].kind = StorageEntryKind::File;
        mutable_entries[slot_index].path[..path_len].copy_from_slice(&path[..path_len]);
        mutable_entries[slot_index].path_len = path_len;
        mutable_entries[slot_index].data_handle = rt::memory_create(INITIAL_FILE_CAPACITY, true)?;
        mutable_entries[slot_index].data_capacity = INITIAL_FILE_CAPACITY;
        mutable_entries[slot_index].data_len = 0;
        mutable_entries[slot_index].occupied = true;
        persist_mutable_entries(persistent_store, mutable_entries)?;

        let pair = rt::channel_create()?;
        blob_session.endpoint = pair.first;
        blob_session.source = crate::BlobSource::Mutable;
        blob_session.data_offset = 0;
        blob_session.data_len = 0;
        blob_session.data_handle = mutable_entries[slot_index].data_handle;
        blob_session.entry_index = slot_index;
        blob_session.writable = writable;
        blob_session.occupied = true;
        send_blob_open_reply(
            StorageTag::DirectoryOpenFileReply,
            reply_handle,
            StorageStatus::Ok,
            0,
            Some(pair.second),
        );
        let _ = rt::handle_close(pair.second);
        return Ok(());
    }

    let Some(entry) = entries.iter().find(|entry| entry.matches(&path[..path_len])) else {
        send_blob_open_reply(
            StorageTag::DirectoryOpenFileReply,
            reply_handle,
            StorageStatus::NotFound,
            0,
            None,
        );
        return Ok(());
    };
    if entry.kind != BootStoreEntryKind::Data {
        send_blob_open_reply(
            StorageTag::DirectoryOpenFileReply,
            reply_handle,
            StorageStatus::InvalidPath,
            0,
            None,
        );
        return Ok(());
    }

    let pair = rt::channel_create()?;
    blob_session.endpoint = pair.first;
    blob_session.source = crate::BlobSource::BootStore;
    blob_session.data_offset = entry.data_offset;
    blob_session.data_len = entry.data_len;
    blob_session.data_handle = rt::INVALID_HANDLE;
    blob_session.entry_index = usize::MAX;
    blob_session.writable = false;
    blob_session.occupied = true;
    send_blob_open_reply(
        StorageTag::DirectoryOpenFileReply,
        reply_handle,
        StorageStatus::Ok,
        entry.data_len,
        Some(pair.second),
    );
    let _ = rt::handle_close(pair.second);
    Ok(())
}
