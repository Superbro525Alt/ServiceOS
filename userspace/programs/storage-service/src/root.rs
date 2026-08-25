use rt::{
    RawMessage, STORAGE_MOUNT_PATH_MAX, StorageEntryKind, StorageMountKind, StorageStatus,
    StorageTag,
};
use serviceos_bundle::BootStoreEntryKind;
use serviceos_userspace_runtime as rt;

use crate::{
    BlobSession, DirectorySession, EntrySlot, MAX_BLOB_SESSIONS, MAX_DIRECTORY_SESSIONS,
    MAX_MUTABLE_ENTRIES, MAX_STORAGE_PATH, MountTable, MutableEntry, PersistentStore,
    path::{
        directory_child_from_path, directory_exists, directory_openable, find_mutable_entry,
        is_mutable_path, path_matches_prefix, resolve_mount, subtree_has_entries,
        valid_directory_path,
    },
    persistent::{ensure_boot_root, persist_state},
    util::{
        pack_bytes, send_blob_open_reply, send_directory_open_reply, send_mount_reply,
        send_stat_reply, unpack_bytes,
    },
};

pub(crate) fn handle_root_request(
    mounts: &mut MountTable,
    entries: &[EntrySlot],
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
    blob_sessions: &mut [BlobSession; MAX_BLOB_SESSIONS],
    directory_sessions: &mut [DirectorySession; MAX_DIRECTORY_SESSIONS],
    persistent_store: Option<&mut PersistentStore>,
    message: &RawMessage,
) -> rt::Result<()> {
    match message.tag {
        x if x == StorageTag::OpenRequest as u32 => {
            handle_open_request(mounts, entries, mutable_entries, blob_sessions, message)
        }
        x if x == StorageTag::ListRequest as u32 => {
            handle_list_request(entries, mutable_entries, message)
        }
        x if x == StorageTag::DirectoryListRequest as u32 => {
            handle_directory_list_request(mounts, entries, mutable_entries, message)
        }
        x if x == StorageTag::DirectoryOpenRequest as u32 => handle_directory_open_request(
            mounts,
            entries,
            mutable_entries,
            directory_sessions,
            message,
        ),
        x if x == StorageTag::MountListRequest as u32 => handle_mount_list_request(mounts, message),
        x if x == StorageTag::MountRequest as u32 => handle_mount_request(
            mounts,
            mutable_entries,
            blob_sessions,
            directory_sessions,
            persistent_store,
            message,
        ),
        x if x == StorageTag::UnmountRequest as u32 => handle_unmount_request(
            mounts,
            mutable_entries,
            blob_sessions,
            directory_sessions,
            persistent_store,
            message,
        ),
        x if x == StorageTag::StatRequest as u32 => {
            handle_stat_request(entries, mutable_entries, message)
        }
        x if x == StorageTag::FindRequest as u32 => {
            handle_find_request(entries, mutable_entries, message)
        }
        _ => Ok(()),
    }
}

/// Shared unmount core used by both the IPC handler and the boot selftest.
/// Atomic: the table slot is only cleared after every gate passes.
pub(crate) fn try_unmount(
    mounts: &mut MountTable,
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
    blob_sessions: &[BlobSession; MAX_BLOB_SESSIONS],
    directory_sessions: &[DirectorySession; MAX_DIRECTORY_SESSIONS],
    mut persistent_store: Option<&mut PersistentStore>,
    prefix: &[u8],
    authority: u64,
) -> StorageStatus {
    let Some(index) = rt::storage_find_mount_by_path(mounts, prefix) else {
        return StorageStatus::NotFound;
    };
    if !rt::storage_mount_authority_ok(&mounts[index], authority) {
        return StorageStatus::Denied;
    }
    if prefix.is_empty() {
        // The boot root anchors the namespace and can never be removed.
        return StorageStatus::Denied;
    }

    let open_paths: [&[u8]; MAX_BLOB_SESSIONS + MAX_DIRECTORY_SESSIONS] =
        open_session_paths(blob_sessions, directory_sessions);
    if rt::storage_unmount_busy(&open_paths[..], prefix) {
        return StorageStatus::Busy;
    }

    // Temp/Ephemeral backends drop their contents; Persistent keeps data on disk.
    let drops_content = !matches!(mounts[index].kind, StorageMountKind::Persistent);
    mounts[index].clear();
    if drops_content {
        for entry in mutable_entries.iter_mut() {
            if entry.occupied
                && !entry.persistent
                && prefix.len() <= entry.path_len
                && entry.path[..prefix.len()] == *prefix
            {
                crate::release_mutable_entry(entry);
            }
        }
    }
    ensure_boot_root(mounts);
    #[allow(clippy::needless_option_as_deref)] // reborrow keeps `persistent_store` usable below
    if persist_state(persistent_store.as_deref_mut(), mounts, mutable_entries).is_err() {
        return StorageStatus::Busy;
    }
    StorageStatus::Ok
}

fn open_session_paths<'a>(
    blob_sessions: &'a [BlobSession; MAX_BLOB_SESSIONS],
    directory_sessions: &'a [DirectorySession; MAX_DIRECTORY_SESSIONS],
) -> [&'a [u8]; MAX_BLOB_SESSIONS + MAX_DIRECTORY_SESSIONS] {
    let mut paths: [&[u8]; MAX_BLOB_SESSIONS + MAX_DIRECTORY_SESSIONS] =
        [&[]; MAX_BLOB_SESSIONS + MAX_DIRECTORY_SESSIONS];
    for (index, session) in blob_sessions.iter().enumerate() {
        paths[index] = &session.path[..if session.occupied {
            session.path_len
        } else {
            0
        }];
    }
    for (index, session) in directory_sessions.iter().enumerate() {
        paths[MAX_BLOB_SESSIONS + index] = &session.path[..if session.occupied {
            session.path_len
        } else {
            0
        }];
    }
    paths
}

fn handle_mount_request(
    mounts: &mut MountTable,
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
    blob_sessions: &mut [BlobSession; MAX_BLOB_SESSIONS],
    directory_sessions: &mut [DirectorySession; MAX_DIRECTORY_SESSIONS],
    persistent_store: Option<&mut PersistentStore>,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 4 || message.handle_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let path_len = message.words[0] as usize;
    let kind = match message.words[1] as u32 {
        x if x == StorageMountKind::Persistent as u32 => StorageMountKind::Persistent,
        x if x == StorageMountKind::Ephemeral as u32 => StorageMountKind::Ephemeral,
        x if x == StorageMountKind::Temp as u32 => StorageMountKind::Temp,
        _ => StorageMountKind::Boot,
    };
    let flags = message.words[2];
    let authority = message.words[3];
    let mut path = [0u8; MAX_STORAGE_PATH];
    if unpack_bytes(
        &message.words[4..message.word_count as usize],
        path_len,
        &mut path,
    )
    .is_err()
        || path_len > path.len()
    {
        send_mount_reply(
            reply_handle,
            StorageTag::MountReply,
            StorageStatus::InvalidPath,
            0,
        );
        return Ok(());
    }
    let path = &path[..path_len];

    // Boot-kind mounts are reserved for the immutable root namespace.
    if kind == StorageMountKind::Boot {
        send_mount_reply(
            reply_handle,
            StorageTag::MountReply,
            StorageStatus::Denied,
            0,
        );
        return Ok(());
    }

    match rt::storage_mount_add(mounts, path, kind, flags, authority) {
        Ok(slot) => {
            if persist_state(persistent_store, mounts, mutable_entries).is_err() {
                mounts[slot].clear();
                ensure_boot_root(mounts);
                send_mount_reply(reply_handle, StorageTag::MountReply, StorageStatus::Busy, 0);
                return Ok(());
            }
            let _ = rt::write_logf(
                "storage",
                format_args!(
                    "mount added prefix-len={} kind={} slot={}",
                    path_len, kind as u32, slot
                ),
            );
            send_mount_reply(
                reply_handle,
                StorageTag::MountReply,
                StorageStatus::Ok,
                slot,
            );
        }
        Err(status) => {
            let _ = (blob_sessions, directory_sessions);
            send_mount_reply(reply_handle, StorageTag::MountReply, status, 0);
        }
    }
    Ok(())
}

fn handle_unmount_request(
    mounts: &mut MountTable,
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
    blob_sessions: &mut [BlobSession; MAX_BLOB_SESSIONS],
    directory_sessions: &mut [DirectorySession; MAX_DIRECTORY_SESSIONS],
    persistent_store: Option<&mut PersistentStore>,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 2 || message.handle_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let path_len = message.words[0] as usize;
    let authority = message.words[1];
    let mut path = [0u8; MAX_STORAGE_PATH];
    if unpack_bytes(
        &message.words[2..message.word_count as usize],
        path_len,
        &mut path,
    )
    .is_err()
    {
        send_mount_reply(
            reply_handle,
            StorageTag::UnmountReply,
            StorageStatus::InvalidPath,
            0,
        );
        return Ok(());
    }

    let status = try_unmount(
        mounts,
        mutable_entries,
        blob_sessions,
        directory_sessions,
        persistent_store,
        &path[..path_len],
        authority,
    );
    let _ = rt::write_logf(
        "storage",
        format_args!(
            "unmount attempt prefix-len={} status={}",
            path_len, status as u32
        ),
    );
    send_mount_reply(reply_handle, StorageTag::UnmountReply, status, 0);
    Ok(())
}

fn handle_stat_request(
    entries: &[EntrySlot],
    mutable_entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 1 || message.handle_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let path_len = message.words[0] as usize;
    let mut path = [0u8; MAX_STORAGE_PATH];
    if unpack_bytes(
        &message.words[1..message.word_count as usize],
        path_len,
        &mut path,
    )
    .is_err()
    {
        send_stat_reply(
            reply_handle,
            StorageStatus::InvalidPath,
            StorageEntryKind::File,
            0,
        );
        return Ok(());
    }
    let path = &path[..path_len];

    if let Some(index) = find_mutable_entry(mutable_entries, path) {
        send_stat_reply(
            reply_handle,
            StorageStatus::Ok,
            mutable_entries[index].kind,
            mutable_entries[index].data_len,
        );
        return Ok(());
    }
    if let Some(entry) = entries.iter().find(|entry| entry.matches(path)) {
        // Boot store carries flat file blobs (data/executable/manifest).
        let _ = entry.kind;
        send_stat_reply(
            reply_handle,
            StorageStatus::Ok,
            StorageEntryKind::File,
            entry.data_len,
        );
        return Ok(());
    }
    if valid_directory_path(path) && directory_exists(entries, mutable_entries, path) {
        send_stat_reply(
            reply_handle,
            StorageStatus::Ok,
            StorageEntryKind::Directory,
            0,
        );
        return Ok(());
    }
    send_stat_reply(
        reply_handle,
        StorageStatus::NotFound,
        StorageEntryKind::File,
        0,
    );
    Ok(())
}

fn handle_find_request(
    entries: &[EntrySlot],
    mutable_entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 3 || message.handle_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let cursor = message.words[0] as usize;
    let root_len = message.words[1] as usize;
    let pattern_len = message.words[2] as usize;
    let mut root_buf = [0u8; MAX_STORAGE_PATH];
    let mut pattern_buf = [0u8; MAX_STORAGE_PATH];
    let payload = &message.words[3..message.word_count as usize];
    if unpack_bytes(payload, root_len.min(root_buf.len()), &mut root_buf).is_err() {
        return Ok(());
    }
    let tail = root_len.div_ceil(8);
    if unpack_bytes(
        payload.get(tail..).unwrap_or(&[]),
        pattern_len.min(pattern_buf.len()),
        &mut pattern_buf,
    )
    .is_err()
    {
        return Ok(());
    }
    let root = &root_buf[..root_len.min(root_buf.len())];
    let pattern_storage = [b'*'; 1];
    let pattern: &[u8] = if pattern_len == 0 {
        &pattern_storage
    } else {
        &pattern_buf[..pattern_len]
    };

    let mut reply = RawMessage::empty(StorageTag::FindReply as u32);
    reply.word_count = 3;
    reply.words[0] = StorageStatus::End as u32 as u64;
    reply.words[1] = cursor as u64;
    reply.words[2] = StorageEntryKind::File as u32 as u64;

    let mut seen = 0usize;
    let candidates = entries
        .iter()
        .map(|entry| (&entry.path[..entry.path_len], StorageEntryKind::File))
        .chain(
            mutable_entries
                .iter()
                .filter(|entry| entry.occupied)
                .map(|entry| (&entry.path[..entry.path_len], entry.kind)),
        );
    for (path, kind) in candidates {
        if !rt::storage_find_entry_matches(root, pattern, path) {
            continue;
        }
        if seen == cursor {
            reply.words[0] = StorageStatus::Ok as u32 as u64;
            reply.words[1] = (seen + 1) as u64;
            reply.words[2] = kind as u32 as u64;
            reply.word_count += pack_bytes(path, &mut reply.words[3..])?;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
            return Ok(());
        }
        seen += 1;
    }

    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

fn handle_open_request(
    mounts: &MountTable,
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
    if unpack_bytes(
        &message.words[1..message.word_count as usize],
        path_len,
        &mut path,
    )
    .is_err()
    {
        return Ok(());
    }
    let path = &path[..path_len];
    let reply_handle = message.handles[0];

    if let Some(index) = find_mutable_entry(mutable_entries, path) {
        if mutable_entries[index].kind != StorageEntryKind::File {
            send_blob_open_reply(
                StorageTag::OpenReply,
                reply_handle,
                StorageStatus::InvalidPath,
                0,
                None,
            );
            return Ok(());
        }
        if let Some(session) = sessions.iter_mut().find(|session| !session.occupied) {
            let pair = rt::channel_create()?;
            *session = stamp_blob_session(
                mounts,
                pair.first,
                path,
                crate::BlobSource::Mutable,
                0,
                mutable_entries[index].data_len,
                mutable_entries[index].data_handle,
                index,
                false,
            );
            send_blob_open_reply(
                StorageTag::OpenReply,
                reply_handle,
                StorageStatus::Ok,
                mutable_entries[index].data_len,
                Some(pair.second),
            );
            let _ = rt::handle_close(pair.second);
        } else {
            send_blob_open_reply(
                StorageTag::OpenReply,
                reply_handle,
                StorageStatus::Busy,
                0,
                None,
            );
        }
        return Ok(());
    }

    let Some(entry) = entries.iter().find(|entry| entry.matches(path)) else {
        send_blob_open_reply(
            StorageTag::OpenReply,
            reply_handle,
            StorageStatus::NotFound,
            0,
            None,
        );
        return Ok(());
    };
    if !matches!(
        entry.kind,
        BootStoreEntryKind::Data | BootStoreEntryKind::Executable
    ) {
        send_blob_open_reply(
            StorageTag::OpenReply,
            reply_handle,
            StorageStatus::InvalidPath,
            0,
            None,
        );
        return Ok(());
    }

    let Some(session) = sessions.iter_mut().find(|session| !session.occupied) else {
        send_blob_open_reply(
            StorageTag::OpenReply,
            reply_handle,
            StorageStatus::Busy,
            0,
            None,
        );
        return Ok(());
    };
    let pair = rt::channel_create()?;
    *session = stamp_blob_session(
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

#[allow(clippy::too_many_arguments)]
pub(crate) fn stamp_blob_session(
    mounts: &MountTable,
    endpoint: rt::Handle,
    path: &[u8],
    source: crate::BlobSource,
    data_offset: usize,
    data_len: usize,
    data_handle: rt::Handle,
    entry_index: usize,
    writable: bool,
) -> BlobSession {
    let mut session = BlobSession::empty();
    session.endpoint = endpoint;
    session.source = source;
    session.data_offset = data_offset;
    session.data_len = data_len;
    session.data_handle = data_handle;
    session.entry_index = entry_index;
    session.path[..path.len()].copy_from_slice(path);
    session.path_len = path.len();
    if let Some(mount) = resolve_mount(mounts, path) {
        session.mount_path[..mount.path_len].copy_from_slice(&mount.path[..mount.path_len]);
        session.mount_path_len = mount.path_len;
    }
    session.writable = writable;
    session.occupied = true;
    session
}

pub(crate) fn handle_directory_open_request(
    mounts: &mut MountTable,
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
    if unpack_bytes(
        &message.words[2..message.word_count as usize],
        path_len,
        &mut path,
    )
    .is_err()
    {
        return Ok(());
    }
    let path = &path[..path_len];
    let reply_handle = message.handles[0];

    if !valid_directory_path(path) {
        send_directory_open_reply(reply_handle, StorageStatus::InvalidPath, None);
        return Ok(());
    }

    if !directory_openable(mounts, entries, mutable_entries, path) {
        send_directory_open_reply(reply_handle, StorageStatus::NotFound, None);
        return Ok(());
    }
    if writable && !is_mutable_path(mounts, path) {
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
    if let Some(mount) = resolve_mount(mounts, path) {
        session.mount_path_len = mount.path_len.min(STORAGE_MOUNT_PATH_MAX);
        session.mount_path[..session.mount_path_len]
            .copy_from_slice(&mount.path[..session.mount_path_len]);
    }
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
    if unpack_bytes(
        &message.words[2..message.word_count as usize],
        prefix_len,
        &mut prefix,
    )
    .is_err()
    {
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
    for entry in mutable_entries.iter().filter(|entry| {
        entry.occupied && path_matches_prefix(&entry.path[..entry.path_len], prefix)
    }) {
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

fn handle_mount_list_request(mounts: &MountTable, message: &RawMessage) -> rt::Result<()> {
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

    let mut occupied: [&rt::StorageMount; rt::STORAGE_MOUNT_TABLE_MAX] =
        [&mounts[0]; rt::STORAGE_MOUNT_TABLE_MAX];
    let mut count = 0usize;
    for mount in mounts.iter().filter(|mount| mount.occupied) {
        occupied[count] = mount;
        count += 1;
    }
    if let Some(mount) = occupied.get(cursor).filter(|_| cursor < count) {
        reply.words[0] = StorageStatus::Ok as u32 as u64;
        reply.words[1] = (cursor + 1) as u64;
        reply.words[2] = mount.kind as u32 as u64;
        reply.words[3] = mount.flags;
        reply.words[4] = mount.path_len as u64;
        reply.word_count += pack_bytes(&mount.path[..mount.path_len], &mut reply.words[5..])?;
    }

    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

fn handle_directory_list_request(
    mounts: &MountTable,
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
    if unpack_bytes(
        &message.words[2..message.word_count as usize],
        prefix_len,
        &mut prefix,
    )
    .is_err()
    {
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
        // Virtual roots: seeded defaults plus every mounted backend prefix.
        for mount in mounts
            .iter()
            .filter(|mount| mount.occupied && !mount.path.is_empty())
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
