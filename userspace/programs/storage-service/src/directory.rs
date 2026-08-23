use rt::{RawMessage, StorageEntryKind, StorageStatus, StorageTag};
use serviceos_bundle::BootStoreEntryKind;
use serviceos_userspace_runtime as rt;

use crate::{
    BlobSession, DirectorySession, EntrySlot, INITIAL_FILE_CAPACITY, MAX_BLOB_SESSIONS,
    MAX_DIRECTORY_SESSIONS, MAX_MUTABLE_ENTRIES, MAX_STORAGE_PATH, MountTable, MutableEntry,
    PersistentStore,
    path::{
        boot_directory_exists, compose_child_path, compose_relative_path,
        directory_child_from_path, directory_exists, find_mutable_entry, is_mutable_path,
        mutable_directory_has_children, resolve_mount, subtree_has_entries,
    },
    persistent::{
        persist_state, release_directory_session as release_dir_session, release_mutable_entry,
    },
    util::{pack_bytes, send_blob_open_reply, send_status_only, send_traverse_reply, unpack_bytes},
};

use serviceos_userspace_runtime::{STORAGE_MOUNT_PATH_MAX, StorageMount};

#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_directory_request(
    mounts: &mut MountTable,
    entries: &[EntrySlot],
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
    directory_sessions: &mut [DirectorySession; MAX_DIRECTORY_SESSIONS],
    blob_sessions: &mut [BlobSession; MAX_BLOB_SESSIONS],
    persistent_store: Option<&mut PersistentStore>,
    session_index: usize,
    message: &RawMessage,
) -> rt::Result<()> {
    let session = directory_sessions[session_index];
    if message.tag == StorageTag::CloseRequest as u32 {
        release_dir_session(&mut directory_sessions[session_index]);
        return Ok(());
    }

    // Boundary rule: a session pinned to a mount dies with that mount.
    if rt::storage_find_mount_by_path(mounts, session.mount_prefix()).is_none() {
        let reply_handle = message
            .handles
            .first()
            .copied()
            .unwrap_or(rt::INVALID_HANDLE);
        if reply_handle != rt::INVALID_HANDLE {
            send_status_only(
                reply_handle,
                StorageTag::DirectoryReadReply,
                StorageStatus::NotMounted,
            );
            let _ = rt::handle_close(reply_handle);
        }
        release_dir_session(&mut directory_sessions[session_index]);
        return Ok(());
    }

    match message.tag {
        x if x == StorageTag::DirectoryReadRequest as u32 => {
            handle_directory_read_request(mounts, entries, mutable_entries, &session, message)
        }
        x if x == StorageTag::DirectoryCreateRequest as u32 => handle_directory_create_request(
            mounts,
            mutable_entries,
            persistent_store,
            &session,
            message,
        ),
        x if x == StorageTag::DirectoryRemoveRequest as u32 => handle_directory_remove_request(
            mounts,
            entries,
            mutable_entries,
            persistent_store,
            &session,
            message,
        ),
        x if x == StorageTag::DirectoryOpenFileRequest as u32 => {
            handle_directory_open_file_request(
                mounts,
                entries,
                mutable_entries,
                blob_sessions,
                persistent_store,
                &session,
                message,
            )
        }
        x if x == StorageTag::DirectoryTraverseRequest as u32 => handle_directory_traverse_request(
            mounts,
            entries,
            mutable_entries,
            directory_sessions,
            blob_sessions,
            &session,
            message,
        ),
        _ => Ok(()),
    }
}

fn handle_directory_read_request(
    mounts: &MountTable,
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
        for mount in mounts
            .iter()
            .filter(|mount| mount.occupied && mount.path_len > 0)
        {
            let root: &[u8] = &mount.path[..mount.path_len];
            if subtree_has_entries(entries, mutable_entries, root) {
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
    mounts: &MountTable,
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
        send_status_only(
            reply_handle,
            StorageTag::DirectoryCreateReply,
            StorageStatus::Denied,
        );
        return Ok(());
    }

    let kind = match message.words[0] as u32 {
        x if x == StorageEntryKind::File as u32 => StorageEntryKind::File,
        x if x == StorageEntryKind::Directory as u32 => StorageEntryKind::Directory,
        _ => {
            send_status_only(
                reply_handle,
                StorageTag::DirectoryCreateReply,
                StorageStatus::InvalidPath,
            );
            return Ok(());
        }
    };
    let name_len = message.words[1] as usize;
    let mut name = [0u8; MAX_STORAGE_PATH];
    if unpack_bytes(
        &message.words[2..message.word_count as usize],
        name_len,
        &mut name,
    )
    .is_err()
    {
        send_status_only(
            reply_handle,
            StorageTag::DirectoryCreateReply,
            StorageStatus::InvalidPath,
        );
        return Ok(());
    }
    let Some((path, path_len)) =
        compose_child_path(&session.path[..session.path_len], &name[..name_len], kind)
    else {
        send_status_only(
            reply_handle,
            StorageTag::DirectoryCreateReply,
            StorageStatus::InvalidPath,
        );
        return Ok(());
    };
    if !is_mutable_path(mounts, &path[..path_len]) {
        send_status_only(
            reply_handle,
            StorageTag::DirectoryCreateReply,
            StorageStatus::Denied,
        );
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
        send_status_only(
            reply_handle,
            StorageTag::DirectoryCreateReply,
            StorageStatus::Busy,
        );
        return Ok(());
    };
    *slot = MutableEntry::empty();
    slot.kind = kind;
    slot.path[..path_len].copy_from_slice(&path[..path_len]);
    slot.path_len = path_len;
    slot.persistent =
        resolve_mount(mounts, &path[..path_len]).is_some_and(|mount| mount.persistent());
    slot.occupied = true;
    if kind == StorageEntryKind::File {
        slot.data_handle = rt::memory_create(INITIAL_FILE_CAPACITY, true)?;
        slot.data_capacity = INITIAL_FILE_CAPACITY;
        slot.data_len = 0;
    }
    persist_state(persistent_store, mounts, mutable_entries)?;
    send_status_only(
        reply_handle,
        StorageTag::DirectoryCreateReply,
        StorageStatus::Ok,
    );
    Ok(())
}

fn handle_directory_remove_request(
    mounts: &MountTable,
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
        send_status_only(
            reply_handle,
            StorageTag::DirectoryRemoveReply,
            StorageStatus::Denied,
        );
        return Ok(());
    }

    let name_len = message.words[0] as usize;
    let mut name = [0u8; MAX_STORAGE_PATH];
    if unpack_bytes(
        &message.words[1..message.word_count as usize],
        name_len,
        &mut name,
    )
    .is_err()
    {
        send_status_only(
            reply_handle,
            StorageTag::DirectoryRemoveReply,
            StorageStatus::InvalidPath,
        );
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
            release_mutable_entry(&mut mutable_entries[index]);
            persist_state(persistent_store, mounts, mutable_entries)?;
            send_status_only(
                reply_handle,
                StorageTag::DirectoryRemoveReply,
                StorageStatus::Ok,
            );
            return Ok(());
        }
        if entries.iter().any(|entry| entry.matches(&path[..path_len])) {
            send_status_only(
                reply_handle,
                StorageTag::DirectoryRemoveReply,
                StorageStatus::Denied,
            );
            return Ok(());
        }
    }
    if let Some((path, path_len)) = dir_path {
        if let Some(index) = find_mutable_entry(mutable_entries, &path[..path_len]) {
            if mutable_directory_has_children(mutable_entries, &path[..path_len]) {
                send_status_only(
                    reply_handle,
                    StorageTag::DirectoryRemoveReply,
                    StorageStatus::Busy,
                );
                return Ok(());
            }
            release_mutable_entry(&mut mutable_entries[index]);
            persist_state(persistent_store, mounts, mutable_entries)?;
            send_status_only(
                reply_handle,
                StorageTag::DirectoryRemoveReply,
                StorageStatus::Ok,
            );
            return Ok(());
        }
        if boot_directory_exists(entries, &path[..path_len]) {
            send_status_only(
                reply_handle,
                StorageTag::DirectoryRemoveReply,
                StorageStatus::Denied,
            );
            return Ok(());
        }
    }

    send_status_only(
        reply_handle,
        StorageTag::DirectoryRemoveReply,
        StorageStatus::NotFound,
    );
    Ok(())
}

fn handle_directory_open_file_request(
    mounts: &MountTable,
    entries: &[EntrySlot],
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
    blob_sessions: &mut [BlobSession; MAX_BLOB_SESSIONS],
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
    if unpack_bytes(
        &message.words[3..message.word_count as usize],
        name_len,
        &mut name,
    )
    .is_err()
    {
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
        *blob_session = crate::stamp_blob_session(
            mounts,
            pair.first,
            &path[..path_len],
            crate::BlobSource::Mutable,
            0,
            mutable_entries[index].data_len,
            mutable_entries[index].data_handle,
            index,
            writable,
        );
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
        if !is_mutable_path(mounts, &path[..path_len]) {
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
        mutable_entries[slot_index].persistent = resolve_mount(mounts, &path[..path_len])
            .is_some_and(|mount: &StorageMount| mount.persistent());
        mutable_entries[slot_index].data_handle = rt::memory_create(INITIAL_FILE_CAPACITY, true)?;
        mutable_entries[slot_index].data_capacity = INITIAL_FILE_CAPACITY;
        mutable_entries[slot_index].data_len = 0;
        mutable_entries[slot_index].occupied = true;
        persist_state(persistent_store, mounts, mutable_entries)?;

        let pair = rt::channel_create()?;
        *blob_session = crate::stamp_blob_session(
            mounts,
            pair.first,
            &path[..path_len],
            crate::BlobSource::Mutable,
            0,
            0,
            mutable_entries[slot_index].data_handle,
            slot_index,
            writable,
        );
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

    let Some(entry) = entries
        .iter()
        .find(|entry| entry.matches(&path[..path_len]))
    else {
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
    *blob_session = crate::stamp_blob_session(
        mounts,
        pair.first,
        &path[..path_len],
        crate::BlobSource::BootStore,
        entry.data_offset,
        entry.data_len,
        rt::INVALID_HANDLE,
        usize::MAX,
        false,
    );
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

fn handle_directory_traverse_request(
    mounts: &mut MountTable,
    entries: &[EntrySlot],
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
    directory_sessions: &mut [DirectorySession; MAX_DIRECTORY_SESSIONS],
    blob_sessions: &mut [BlobSession; MAX_BLOB_SESSIONS],
    session: &DirectorySession,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 3 || message.handle_count < 1 {
        return Ok(());
    }

    let reply_handle = message.handles[0];
    let path_len = message.words[0] as usize;
    let want_directory = message.words[1] != 0;
    let writable = message.words[2] != 0;
    let mut path = [0u8; MAX_STORAGE_PATH];
    if unpack_bytes(
        &message.words[3..message.word_count as usize],
        path_len,
        &mut path,
    )
    .is_err()
    {
        send_traverse_reply(
            reply_handle,
            StorageStatus::InvalidPath,
            StorageEntryKind::File,
            0,
            None,
        );
        return Ok(());
    }

    let kind = if want_directory {
        StorageEntryKind::Directory
    } else {
        StorageEntryKind::File
    };
    let Some((resolved, resolved_len)) =
        compose_relative_path(&session.path[..session.path_len], &path[..path_len], kind)
    else {
        send_traverse_reply(reply_handle, StorageStatus::InvalidPath, kind, 0, None);
        return Ok(());
    };

    if writable && (!session.writable || !is_mutable_path(mounts, &resolved[..resolved_len])) {
        send_traverse_reply(reply_handle, StorageStatus::Denied, kind, 0, None);
        return Ok(());
    }

    if want_directory {
        if !directory_exists(entries, mutable_entries, &resolved[..resolved_len]) {
            send_traverse_reply(reply_handle, StorageStatus::NotFound, kind, 0, None);
            return Ok(());
        }
        let Some(dir_session) = open_directory_session(
            mounts,
            directory_sessions,
            &resolved[..resolved_len],
            writable,
        )?
        else {
            send_traverse_reply(reply_handle, StorageStatus::Busy, kind, 0, None);
            return Ok(());
        };
        send_traverse_reply(reply_handle, StorageStatus::Ok, kind, 0, Some(dir_session));
        return Ok(());
    }

    let (status, opened_file) = open_file_session(
        mounts,
        entries,
        mutable_entries,
        blob_sessions,
        &resolved[..resolved_len],
        writable,
    )?;
    let Some((blob_handle, len)) = opened_file else {
        send_traverse_reply(reply_handle, status, kind, 0, None);
        return Ok(());
    };
    send_traverse_reply(
        reply_handle,
        StorageStatus::Ok,
        kind,
        len,
        Some(blob_handle),
    );
    Ok(())
}

fn open_directory_session(
    mounts: &MountTable,
    directory_sessions: &mut [DirectorySession; MAX_DIRECTORY_SESSIONS],
    path: &[u8],
    writable: bool,
) -> rt::Result<Option<rt::Handle>> {
    let Some(dir_session) = directory_sessions.iter_mut().find(|entry| !entry.occupied) else {
        return Ok(None);
    };
    let pair = rt::channel_create()?;
    dir_session.endpoint = pair.first;
    dir_session.path[..path.len()].copy_from_slice(path);
    dir_session.path_len = path.len();
    if let Some(mount) = resolve_mount(mounts, path) {
        dir_session.mount_path_len = mount.path_len.min(STORAGE_MOUNT_PATH_MAX);
        dir_session.mount_path[..dir_session.mount_path_len]
            .copy_from_slice(&mount.path[..dir_session.mount_path_len]);
    }
    dir_session.writable = writable;
    dir_session.occupied = true;
    Ok(Some(pair.second))
}

fn open_file_session(
    mounts: &MountTable,
    entries: &[EntrySlot],
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
    blob_sessions: &mut [crate::BlobSession; crate::MAX_BLOB_SESSIONS],
    path: &[u8],
    writable: bool,
) -> rt::Result<(StorageStatus, Option<(rt::Handle, usize)>)> {
    if let Some(index) = find_mutable_entry(mutable_entries, path) {
        if mutable_entries[index].kind != StorageEntryKind::File {
            return Ok((StorageStatus::InvalidPath, None));
        }
        let Some(blob_session) = blob_sessions.iter_mut().find(|entry| !entry.occupied) else {
            return Ok((StorageStatus::Busy, None));
        };
        let pair = rt::channel_create()?;
        *blob_session = crate::stamp_blob_session(
            mounts,
            pair.first,
            path,
            crate::BlobSource::Mutable,
            0,
            mutable_entries[index].data_len,
            mutable_entries[index].data_handle,
            index,
            writable,
        );
        return Ok((
            StorageStatus::Ok,
            Some((pair.second, mutable_entries[index].data_len)),
        ));
    }

    if writable {
        return Ok((StorageStatus::Denied, None));
    }
    let Some(entry) = entries.iter().find(|entry| entry.matches(path)) else {
        return Ok((StorageStatus::NotFound, None));
    };
    if entry.kind != BootStoreEntryKind::Data {
        return Ok((StorageStatus::InvalidPath, None));
    }
    let Some(blob_session) = blob_sessions.iter_mut().find(|entry| !entry.occupied) else {
        return Ok((StorageStatus::Busy, None));
    };
    let pair = rt::channel_create()?;
    *blob_session = crate::stamp_blob_session(
        mounts,
        pair.first,
        path,
        crate::BlobSource::BootStore,
        entry.data_offset,
        entry.data_len,
        rt::INVALID_HANDLE,
        usize::MAX,
        false,
    );
    Ok((StorageStatus::Ok, Some((pair.second, entry.data_len))))
}
