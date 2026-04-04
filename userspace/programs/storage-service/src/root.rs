use serviceos_bundle::BootStoreEntryKind;
use serviceos_userspace_runtime as rt;
use rt::{RawMessage, StorageEntryKind, StorageMountKind, StorageStatus, StorageTag};

use crate::{
    path::{
        directory_child_from_path, directory_exists, find_mutable_entry,
        is_mutable_directory_path, path_matches_prefix, valid_directory_path,
        mutable_root_has_materialized_children,
    },
    util::{pack_bytes, send_blob_open_reply, send_directory_open_reply, unpack_bytes},
    BlobSession, DirectorySession, EntrySlot, MutableEntry, MAX_BLOB_SESSIONS,
    MAX_DIRECTORY_SESSIONS, MAX_MUTABLE_ENTRIES, MAX_STORAGE_PATH, MUTABLE_ROOTS,
};

pub(crate) fn handle_root_request(
    entries: &[EntrySlot],
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
    blob_sessions: &mut [BlobSession; MAX_BLOB_SESSIONS],
    directory_sessions: &mut [DirectorySession; MAX_DIRECTORY_SESSIONS],
    _persistent_store: Option<&mut crate::PersistentStore>,
    message: &RawMessage,
) -> rt::Result<()> {
    match message.tag {
        x if x == StorageTag::OpenRequest as u32 => {
            handle_open_request(entries, mutable_entries, blob_sessions, message)
        }
        x if x == StorageTag::ListRequest as u32 => handle_list_request(entries, mutable_entries, message),
        x if x == StorageTag::DirectoryListRequest as u32 => {
            handle_directory_list_request(entries, mutable_entries, message)
        }
        x if x == StorageTag::DirectoryOpenRequest as u32 => {
            handle_directory_open_request(entries, mutable_entries, directory_sessions, message)
        }
        x if x == StorageTag::MountListRequest as u32 => handle_mount_list_request(message),
        _ => Ok(()),
    }
}

const STORAGE_MOUNTS: [(&[u8], StorageMountKind, u64); 5] = [
    (b"", StorageMountKind::Boot, 0),
    (b"home/", StorageMountKind::Persistent, 0b11),
    (b"state/", StorageMountKind::Persistent, 0b11),
    (b"projects/", StorageMountKind::Persistent, 0b11),
    (b"tmp/", StorageMountKind::Ephemeral, 0b01),
];

fn handle_open_request(
    entries: &[EntrySlot],
    mutable_entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
    sessions: &mut [BlobSession; MAX_BLOB_SESSIONS],
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 1 || message.handle_count < 1 {
        return Ok(());
    }

    let path_len = message.words[0] as usize;
    let mut path = [0u8; MAX_STORAGE_PATH];
    if unpack_bytes(&message.words[1..message.word_count as usize], path_len, &mut path).is_err() {
        return Ok(());
    }
    let path = &path[..path_len];
    let reply_handle = message.handles[0];

    if let Some(index) = find_mutable_entry(mutable_entries, path) {
        if mutable_entries[index].kind != StorageEntryKind::File {
            send_blob_open_reply(StorageTag::OpenReply, reply_handle, StorageStatus::InvalidPath, 0, None);
            return Ok(());
        }
        if let Some(session) = sessions.iter_mut().find(|session| !session.occupied) {
            let pair = rt::channel_create()?;
            session.endpoint = pair.first;
            session.source = crate::BlobSource::Mutable;
            session.data_offset = 0;
            session.data_len = mutable_entries[index].data_len;
            session.data_handle = mutable_entries[index].data_handle;
            session.entry_index = index;
            session.writable = false;
            session.occupied = true;
            send_blob_open_reply(
                StorageTag::OpenReply,
                reply_handle,
                StorageStatus::Ok,
                mutable_entries[index].data_len,
                Some(pair.second),
            );
            let _ = rt::handle_close(pair.second);
        } else {
            send_blob_open_reply(StorageTag::OpenReply, reply_handle, StorageStatus::Busy, 0, None);
        }
        return Ok(());
    }

    let Some(entry) = entries.iter().find(|entry| entry.matches(path)) else {
        send_blob_open_reply(StorageTag::OpenReply, reply_handle, StorageStatus::NotFound, 0, None);
        return Ok(());
    };
    if !matches!(entry.kind, BootStoreEntryKind::Data | BootStoreEntryKind::Executable) {
        send_blob_open_reply(StorageTag::OpenReply, reply_handle, StorageStatus::InvalidPath, 0, None);
        return Ok(());
    }

    let Some(session) = sessions.iter_mut().find(|session| !session.occupied) else {
        send_blob_open_reply(StorageTag::OpenReply, reply_handle, StorageStatus::Busy, 0, None);
        return Ok(());
    };
    let pair = rt::channel_create()?;
    session.endpoint = pair.first;
    session.source = crate::BlobSource::BootStore;
    session.data_offset = entry.data_offset;
    session.data_len = entry.data_len;
    session.data_handle = rt::INVALID_HANDLE;
    session.entry_index = usize::MAX;
    session.writable = false;
    session.occupied = true;
    send_blob_open_reply(
        StorageTag::OpenReply,
        reply_handle,
        StorageStatus::Ok,
        entry.data_len,
        Some(pair.second),
    );
    let _ = rt::handle_close(pair.second);
    Ok(())
}

fn handle_directory_open_request(
    entries: &[EntrySlot],
    mutable_entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
    sessions: &mut [DirectorySession; MAX_DIRECTORY_SESSIONS],
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 2 || message.handle_count < 1 {
        return Ok(());
    }

    let path_len = message.words[0] as usize;
    let writable = message.words[1] != 0;
    let mut path = [0u8; MAX_STORAGE_PATH];
    if unpack_bytes(&message.words[2..message.word_count as usize], path_len, &mut path).is_err() {
        return Ok(());
    }
    let path = &path[..path_len];
    let reply_handle = message.handles[0];

    if !valid_directory_path(path) {
        send_directory_open_reply(reply_handle, StorageStatus::InvalidPath, None);
        return Ok(());
    }

    if !directory_exists(entries, mutable_entries, path) {
        send_directory_open_reply(reply_handle, StorageStatus::NotFound, None);
        return Ok(());
    }
    if writable && !is_mutable_directory_path(path) {
        send_directory_open_reply(reply_handle, StorageStatus::Denied, None);
        return Ok(());
    }

    let Some(session) = sessions.iter_mut().find(|session| !session.occupied) else {
        send_directory_open_reply(reply_handle, StorageStatus::Busy, None);
        return Ok(());
    };
    let pair = rt::channel_create()?;
    session.endpoint = pair.first;
    session.path[..path.len()].copy_from_slice(path);
    session.path_len = path.len();
    session.writable = writable;
    session.occupied = true;
    send_directory_open_reply(reply_handle, StorageStatus::Ok, Some(pair.second));
    let _ = rt::handle_close(pair.second);
    Ok(())
}

fn handle_list_request(
    entries: &[EntrySlot],
    mutable_entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 2 || message.handle_count < 1 {
        return Ok(());
    }

    let list_index = message.words[0] as usize;
    let prefix_len = message.words[1] as usize;
    let mut prefix = [0u8; MAX_STORAGE_PATH];
    if unpack_bytes(&message.words[2..message.word_count as usize], prefix_len, &mut prefix).is_err() {
        return Ok(());
    }
    let prefix = &prefix[..prefix_len];

    let reply_handle = message.handles[0];
    let mut reply = RawMessage::empty(StorageTag::ListReply as u32);
    reply.word_count = 3;
    reply.words[0] = StorageStatus::End as u32 as u64;
    reply.words[1] = 0;
    reply.words[2] = 0;

    let mut current = 0usize;
    for entry in entries.iter().filter(|entry| entry.matches_prefix(prefix)) {
        if current == list_index {
            reply.words[0] = StorageStatus::Ok as u32 as u64;
            reply.words[1] = entry.kind as u32 as u64;
            reply.words[2] = entry.path_len as u64;
            reply.word_count += pack_bytes(&entry.path[..entry.path_len], &mut reply.words[3..])?;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
            return Ok(());
        }
        current += 1;
    }
    for entry in mutable_entries
        .iter()
        .filter(|entry| entry.occupied && path_matches_prefix(&entry.path[..entry.path_len], prefix))
    {
        if current == list_index {
            reply.words[0] = StorageStatus::Ok as u32 as u64;
            reply.words[1] = entry.kind as u32 as u64;
            reply.words[2] = entry.path_len as u64;
            reply.word_count += pack_bytes(&entry.path[..entry.path_len], &mut reply.words[3..])?;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
            return Ok(());
        }
        current += 1;
    }

    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

fn handle_mount_list_request(message: &RawMessage) -> rt::Result<()> {
    if message.word_count < 1 || message.handle_count < 1 {
        return Ok(());
    }

    let cursor = message.words[0] as usize;
    let reply_handle = message.handles[0];
    let mut reply = RawMessage::empty(StorageTag::MountListReply as u32);
    reply.word_count = 5;
    reply.words[0] = StorageStatus::End as u32 as u64;
    reply.words[1] = cursor as u64;
    reply.words[2] = StorageMountKind::Boot as u32 as u64;
    reply.words[3] = 0;
    reply.words[4] = 0;

    if let Some((index, (path, kind, flags))) =
        STORAGE_MOUNTS.iter().enumerate().skip(cursor).next()
    {
        reply.words[0] = StorageStatus::Ok as u32 as u64;
        reply.words[1] = (index + 1) as u64;
        reply.words[2] = *kind as u32 as u64;
        reply.words[3] = *flags;
        reply.words[4] = path.len() as u64;
        reply.word_count += pack_bytes(path, &mut reply.words[5..])?;
    }

    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

fn handle_directory_list_request(
    entries: &[EntrySlot],
    mutable_entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 2 || message.handle_count < 1 {
        return Ok(());
    }

    let cursor = message.words[0] as usize;
    let prefix_len = message.words[1] as usize;
    let mut prefix = [0u8; MAX_STORAGE_PATH];
    if unpack_bytes(&message.words[2..message.word_count as usize], prefix_len, &mut prefix).is_err() {
        return Ok(());
    }
    let prefix = &prefix[..prefix_len];
    let reply_handle = message.handles[0];

    let mut reply = RawMessage::empty(StorageTag::DirectoryListReply as u32);
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
        if let Some((child_path, child_kind)) = directory_child_from_path(&entry.path[..entry.path_len], prefix) {
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
