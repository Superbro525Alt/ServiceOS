use rt::{
    Handle, RawMessage, STORAGE_MOUNT_PATH_MAX, StorageEntryKind, StorageMountKind, StorageStatus,
    StorageTag,
};
use serviceos_bundle::BootStoreEntryKind;
use serviceos_userspace_runtime as rt;

use crate::{
    BlobSession, DirectorySession, EntrySlot, MAX_BLOB_SESSIONS, MAX_DIRECTORY_SESSIONS,
    MAX_MUTABLE_ENTRIES, MAX_STORAGE_PATH, MountTable, MutableEntry, PersistentStore,
    index::{
        GREP_REQUEST_TAG, SEARCH_REQUEST_TAG, SearchIndex, handle_grep_request,
        handle_search_request, index_tail_words,
    },
    path::{
        directory_child_from_path, directory_exists, directory_openable, find_mutable_entry,
        is_mutable_path, path_matches_prefix, resolve_mount, subtree_has_entries,
        valid_directory_path,
    },
    persistent::{ensure_boot_root, persist_state},
    util::{
        pack_bytes, send_blob_open_reply, send_directory_open_reply, send_mount_reply,
        send_stat_reply, send_status_only, unpack_bytes,
    },
};

pub(crate) fn handle_root_request(
    mounts: &mut MountTable,
    bootstore: Handle,
    entries: &[EntrySlot],
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
    blob_sessions: &mut [BlobSession; MAX_BLOB_SESSIONS],
    directory_sessions: &mut [DirectorySession; MAX_DIRECTORY_SESSIONS],
    persistent_store: Option<&mut PersistentStore>,
    search_index: &mut SearchIndex,
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
            handle_stat_request(entries, mutable_entries, search_index, message)
        }
        x if x == StorageTag::FindRequest as u32 => {
            handle_find_request(entries, mutable_entries, message)
        }
        x if x == SEARCH_REQUEST_TAG => {
            handle_search_request(entries, mutable_entries, search_index, message)
        }
        x if x == GREP_REQUEST_TAG => {
            handle_grep_request(bootstore, entries, mutable_entries, search_index, message)
        }
        x if x == crate::fsck::FSCK_REQUEST_TAG => crate::fsck::handle_fsck_request(
            mounts,
            mutable_entries,
            search_index,
            persistent_store,
            message,
        ),
        x if x == StorageTag::RenameRequest as u32 => handle_rename_request(
            mounts,
            entries,
            mutable_entries,
            persistent_store,
            search_index,
            message,
        ),
        _ => Ok(()),
    }
}

/// Shared rename/move core used by the IPC handler and host tests.
/// Atomic: the live entry table is only rewritten after every gate passes.
/// A same-directory rename and a cross-directory move are the same
/// operation here; a directory source moves its whole subtree. The boot
/// store is read-only, so sources that exist only there and destinations
/// that would shadow a boot entry are denied, and destination collisions
/// with live entries are rejected without overwrite.
pub(crate) fn try_rename_entry(
    mounts: &MountTable,
    entries: &[EntrySlot],
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
    search_index: &mut SearchIndex,
    source: &[u8],
    dest: &[u8],
    now: u64,
) -> StorageStatus {
    if source.is_empty() || dest.is_empty() {
        return StorageStatus::InvalidPath;
    }
    // Kind is carried by the trailing-slash convention; both endpoints must
    // agree on it.
    let source_is_dir = source.ends_with(b"/");
    if dest.ends_with(b"/") != source_is_dir {
        return StorageStatus::InvalidPath;
    }
    let source_kind = if source_is_dir {
        StorageEntryKind::Directory
    } else {
        StorageEntryKind::File
    };
    if source == dest {
        return StorageStatus::Ok;
    }

    let Some(source_index) = find_mutable_entry(mutable_entries, source) else {
        if entries.iter().any(|entry| entry.matches(source)) {
            return StorageStatus::Denied;
        }
        return StorageStatus::NotFound;
    };
    if mutable_entries[source_index].kind != source_kind {
        return StorageStatus::InvalidPath;
    }

    // Capability gate: both endpoints must sit on writable mounts.
    if !is_mutable_path(mounts, source) || !is_mutable_path(mounts, dest) {
        return StorageStatus::Denied;
    }

    // No overwrite: the destination must not collide with a live mutable
    // entry nor shadow a read-only boot-store entry.
    if find_mutable_entry(mutable_entries, dest).is_some() {
        return StorageStatus::AlreadyExists;
    }
    if entries.iter().any(|entry| entry.matches(dest)) {
        return StorageStatus::Denied;
    }

    // The destination parent must resolve (root, mount root, boot or
    // mutable directory). A directory destination's parent excludes the
    // destination's own trailing component.
    let dest_parent = match dest
        .strip_suffix(b"/")
        .unwrap_or(dest)
        .iter()
        .rposition(|byte| *byte == b'/')
    {
        Some(position) => &dest[..position + 1],
        None => &[],
    };
    if !directory_openable(mounts, entries, mutable_entries, dest_parent) {
        return StorageStatus::NotFound;
    }

    // A directory may not be moved into itself.
    if source_is_dir && dest.len() > source.len() && dest[..source.len()] == *source {
        return StorageStatus::InvalidPath;
    }

    // Subtree capacity and collision plan: every rewritten descendant must
    // fit under the new prefix and must not land on an entry that stays
    // put (mutable or boot).
    if source_is_dir {
        for entry in mutable_entries.iter() {
            if !entry.occupied {
                continue;
            }
            let under = entry.path_len > source.len() && entry.path[..source.len()] == *source;
            if !under {
                continue;
            }
            let suffix = &entry.path[source.len()..entry.path_len];
            let new_len = dest.len() + suffix.len();
            if new_len > MAX_STORAGE_PATH {
                return StorageStatus::InvalidPath;
            }
            let mut new_path = [0u8; MAX_STORAGE_PATH];
            new_path[..dest.len()].copy_from_slice(dest);
            new_path[dest.len()..new_len].copy_from_slice(suffix);
            let new_path = &new_path[..new_len];
            // Only entries that stay put can collide with the moved
            // subtree; subtree members rewrite injectively among
            // themselves.
            let stays_put = |other: &MutableEntry| -> bool {
                other.occupied
                    && !(other.path_len == source.len() && other.path[..other.path_len] == *source)
                    && !(other.path_len > source.len() && other.path[..source.len()] == *source)
            };
            if mutable_entries.iter().any(|other| {
                stays_put(other) && other.path_len == new_len && other.path[..new_len] == *new_path
            }) {
                return StorageStatus::AlreadyExists;
            }
            if entries.iter().any(|boot| boot.matches(new_path)) {
                return StorageStatus::Denied;
            }
        }
    }

    // Apply: rewrite paths in place, keeping data handles, sizes, and
    // kinds; entries adopt the destination mount's persistence (matching
    // what create would have set there).
    let persistent_now = resolve_mount(mounts, dest).is_some_and(|mount| mount.persistent());
    for entry in mutable_entries.iter_mut() {
        if !entry.occupied {
            continue;
        }
        let is_self = entry.path_len == source.len() && entry.path[..entry.path_len] == *source;
        let under =
            source_is_dir && entry.path_len > source.len() && entry.path[..source.len()] == *source;
        if !is_self && !under {
            continue;
        }
        entry
            .path
            .copy_within(source.len()..entry.path_len, dest.len());
        entry.path[..dest.len()].copy_from_slice(dest);
        entry.path_len = dest.len() + (entry.path_len - source.len());
        entry.persistent = persistent_now;
    }

    if source_is_dir {
        search_index.rename_tree(source, dest, now);
    } else {
        search_index.rename(source, dest, now);
    }
    StorageStatus::Ok
}

fn handle_rename_request(
    mounts: &mut MountTable,
    entries: &[EntrySlot],
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
    persistent_store: Option<&mut PersistentStore>,
    search_index: &mut SearchIndex,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.word_count < 3 || message.handle_count < 1 {
        return Ok(());
    }
    let reply_handle = message.handles[0];
    let source_len = message.words[0] as usize;
    let dest_len = message.words[1] as usize;
    let mut source_buf = [0u8; MAX_STORAGE_PATH];
    let mut dest_buf = [0u8; MAX_STORAGE_PATH];
    let payload = &message.words[2..message.word_count as usize];
    if unpack_bytes(payload, source_len, &mut source_buf).is_err() {
        send_status_only(
            reply_handle,
            StorageTag::RenameReply,
            StorageStatus::InvalidPath,
        );
        return Ok(());
    }
    let tail = source_len.div_ceil(8);
    if unpack_bytes(payload.get(tail..).unwrap_or(&[]), dest_len, &mut dest_buf).is_err() {
        send_status_only(
            reply_handle,
            StorageTag::RenameReply,
            StorageStatus::InvalidPath,
        );
        return Ok(());
    }
    let source = &source_buf[..source_len];
    let dest = &dest_buf[..dest_len];
    let now = rt::monotonic_now().unwrap_or(0);
    let status = try_rename_entry(
        mounts,
        entries,
        mutable_entries,
        search_index,
        source,
        dest,
        now,
    );
    if status == StorageStatus::Ok {
        persist_state(persistent_store, mounts, mutable_entries)?;
    }
    send_status_only(reply_handle, StorageTag::RenameReply, status);
    Ok(())
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
    search_index: &SearchIndex,
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
            &[0; 4],
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
            &index_tail_words(search_index),
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
            &index_tail_words(search_index),
        );
        return Ok(());
    }
    if valid_directory_path(path) && directory_exists(entries, mutable_entries, path) {
        send_stat_reply(
            reply_handle,
            StorageStatus::Ok,
            StorageEntryKind::Directory,
            0,
            &index_tail_words(search_index),
        );
        return Ok(());
    }
    send_stat_reply(
        reply_handle,
        StorageStatus::NotFound,
        StorageEntryKind::File,
        0,
        &[0; 4],
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{SearchPlan, plan_search};
    use rt::{STORAGE_MOUNT_FLAG_PERSISTENT, STORAGE_MOUNT_FLAG_WRITABLE, STORAGE_ROOT_AUTHORITY};

    fn seeded_mounts() -> MountTable {
        let mut mounts = [rt::StorageMount::empty(); rt::STORAGE_MOUNT_TABLE_MAX];
        let defaults: [(&[u8], StorageMountKind, u64); 3] = [
            (
                b"home/",
                StorageMountKind::Persistent,
                STORAGE_MOUNT_FLAG_WRITABLE | STORAGE_MOUNT_FLAG_PERSISTENT,
            ),
            (
                b"tmp/",
                StorageMountKind::Ephemeral,
                STORAGE_MOUNT_FLAG_WRITABLE,
            ),
            (b"rom/", StorageMountKind::Temp, 0),
        ];
        for (slot, (path, kind, flags)) in mounts.iter_mut().zip(defaults.iter()) {
            assert!(
                slot.install(path, *kind, *flags, STORAGE_ROOT_AUTHORITY)
                    .is_ok()
            );
        }
        mounts
    }

    fn mutable_entry(path: &[u8], kind: StorageEntryKind, data_len: usize) -> MutableEntry {
        let mut entry = MutableEntry::empty();
        entry.kind = kind;
        entry.path[..path.len()].copy_from_slice(path);
        entry.path_len = path.len();
        entry.persistent = true;
        entry.data_len = data_len;
        entry.occupied = true;
        entry
    }

    fn boot_entry(path: &[u8], data_len: usize) -> EntrySlot {
        let mut entry = EntrySlot::empty();
        entry.kind = BootStoreEntryKind::Data;
        entry.path[..path.len()].copy_from_slice(path);
        entry.path_len = path.len();
        entry.data_offset = 40;
        entry.data_len = data_len;
        entry
    }

    fn insert(
        mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
        path: &[u8],
        kind: StorageEntryKind,
        data_len: usize,
    ) {
        let slot = mutable_entries
            .iter_mut()
            .find(|entry| !entry.occupied)
            .unwrap();
        *slot = mutable_entry(path, kind, data_len);
    }

    fn build_index(
        entries: &[EntrySlot],
        mutable_entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
    ) -> SearchIndex {
        let mut index = SearchIndex::new();
        assert!(index.ensure_built(entries, mutable_entries, 10));
        index
    }

    fn indexed_paths(index: &SearchIndex) -> Vec<Vec<u8>> {
        let mut paths: Vec<Vec<u8>> = index
            .snapshot()
            .iter()
            .map(|entry| entry.path().to_vec())
            .collect();
        paths.sort();
        paths
    }

    #[test]
    fn same_dir_rename_reindexes_and_preserves_metadata() {
        let mounts = seeded_mounts();
        let boot: [EntrySlot; 0] = [];
        let mut mutable = [MutableEntry::empty(); MAX_MUTABLE_ENTRIES];
        insert(&mut mutable, b"home/note.txt", StorageEntryKind::File, 7);
        insert(&mut mutable, b"home/other.txt", StorageEntryKind::File, 3);
        let mut index = build_index(&boot, &mutable);

        let status = try_rename_entry(
            &mounts,
            &boot,
            &mut mutable,
            &mut index,
            b"home/note.txt",
            b"home/renamed.txt",
            77,
        );
        assert_eq!(status, StorageStatus::Ok);

        assert!(find_mutable_entry(&mutable, b"home/note.txt").is_none());
        let moved = find_mutable_entry(&mutable, b"home/renamed.txt").unwrap();
        assert_eq!(mutable[moved].kind, StorageEntryKind::File);
        assert_eq!(mutable[moved].data_len, 7);
        let other = find_mutable_entry(&mutable, b"home/other.txt").unwrap();
        assert_eq!(mutable[other].data_len, 3);

        let plan = plan_search(index.snapshot(), b"", &[b"renamed"], 0, u64::MAX, 0, 80);
        assert_eq!(plan.len, 1);
        let stale = plan_search(index.snapshot(), b"", &[b"note"], 0, u64::MAX, 0, 80);
        assert_eq!(stale.len, 0);
        assert!(!index.stats().1);
        assert_eq!(index.stats().0, 2);
    }

    #[test]
    fn cross_dir_move_reindexes_subtree_and_flips_persistence() {
        let mounts = seeded_mounts();
        let boot: [EntrySlot; 0] = [];
        let mut mutable = [MutableEntry::empty(); MAX_MUTABLE_ENTRIES];
        insert(&mut mutable, b"home/box/", StorageEntryKind::Directory, 0);
        insert(&mut mutable, b"home/box/a.txt", StorageEntryKind::File, 5);
        insert(
            &mut mutable,
            b"home/box/sub/",
            StorageEntryKind::Directory,
            0,
        );
        insert(
            &mut mutable,
            b"home/box/sub/b.txt",
            StorageEntryKind::File,
            2,
        );
        insert(&mut mutable, b"home/keep.txt", StorageEntryKind::File, 9);
        let mut index = build_index(&boot, &mutable);

        let status = try_rename_entry(
            &mounts,
            &boot,
            &mut mutable,
            &mut index,
            b"home/box/",
            b"tmp/moved/",
            80,
        );
        assert_eq!(status, StorageStatus::Ok);

        for old in [
            b"home/box/".as_slice(),
            b"home/box/a.txt".as_slice(),
            b"home/box/sub/".as_slice(),
            b"home/box/sub/b.txt".as_slice(),
        ] {
            assert!(find_mutable_entry(&mutable, old).is_none(), "stale {old:?}");
        }
        let moved_file = find_mutable_entry(&mutable, b"tmp/moved/a.txt").unwrap();
        assert_eq!(mutable[moved_file].kind, StorageEntryKind::File);
        assert_eq!(mutable[moved_file].data_len, 5);
        assert!(!mutable[moved_file].persistent);
        let moved_dir = find_mutable_entry(&mutable, b"tmp/moved/").unwrap();
        assert_eq!(mutable[moved_dir].kind, StorageEntryKind::Directory);
        assert!(find_mutable_entry(&mutable, b"tmp/moved/sub/b.txt").is_some());
        let keeper = find_mutable_entry(&mutable, b"home/keep.txt").unwrap();
        assert!(mutable[keeper].persistent);

        for path in [
            b"tmp/moved/".as_slice(),
            b"tmp/moved/a.txt".as_slice(),
            b"tmp/moved/sub/".as_slice(),
            b"tmp/moved/sub/b.txt".as_slice(),
        ] {
            assert!(
                index.snapshot().iter().any(|entry| entry.path() == path),
                "missing {path:?}"
            );
        }
        assert!(
            !index
                .snapshot()
                .iter()
                .any(|entry| entry.path().starts_with(b"home/box"))
        );
        assert_eq!(index.stats().0, 5);
        assert!(!index.stats().1);
        let in_home = plan_search(index.snapshot(), b"home/", &[b"txt"], 0, u64::MAX, 0, 80);
        assert_eq!(
            plan_paths_of(&in_home, index.snapshot()),
            vec![b"home/keep.txt".to_vec()]
        );
        let in_tmp = plan_search(index.snapshot(), b"tmp/", &[b"txt"], 0, u64::MAX, 0, 80);
        assert_eq!(in_tmp.len, 2);
    }

    #[test]
    fn dest_collision_rejected_without_overwrite() {
        let mounts = seeded_mounts();
        let boot: [EntrySlot; 0] = [];
        let mut mutable = [MutableEntry::empty(); MAX_MUTABLE_ENTRIES];
        insert(&mut mutable, b"home/note.txt", StorageEntryKind::File, 7);
        insert(&mut mutable, b"home/other.txt", StorageEntryKind::File, 3);
        let mut index = build_index(&boot, &mutable);

        let status = try_rename_entry(
            &mounts,
            &boot,
            &mut mutable,
            &mut index,
            b"home/note.txt",
            b"home/other.txt",
            90,
        );
        assert_eq!(status, StorageStatus::AlreadyExists);

        let note = find_mutable_entry(&mutable, b"home/note.txt").unwrap();
        let other = find_mutable_entry(&mutable, b"home/other.txt").unwrap();
        assert_eq!(mutable[note].data_len, 7);
        assert_eq!(mutable[other].data_len, 3);
        assert!(!index.stats().1);
        assert_eq!(indexed_paths(&index).len(), 2);
    }

    #[test]
    fn boot_store_sources_and_destinations_are_denied() {
        let mounts = seeded_mounts();
        let boot = [boot_entry(b"home/boot.txt", 4)];
        let mut mutable = [MutableEntry::empty(); MAX_MUTABLE_ENTRIES];
        insert(&mut mutable, b"home/mine.txt", StorageEntryKind::File, 6);
        let mut index = build_index(&boot, &mutable);

        // Destination shadowing a read-only boot entry.
        let status = try_rename_entry(
            &mounts,
            &boot,
            &mut mutable,
            &mut index,
            b"home/mine.txt",
            b"home/boot.txt",
            90,
        );
        assert_eq!(status, StorageStatus::Denied);
        assert!(find_mutable_entry(&mutable, b"home/mine.txt").is_some());

        // Source that exists only in the boot store.
        let status = try_rename_entry(
            &mounts,
            &boot,
            &mut mutable,
            &mut index,
            b"home/boot.txt",
            b"home/moved.txt",
            90,
        );
        assert_eq!(status, StorageStatus::Denied);
        assert!(find_mutable_entry(&mutable, b"home/moved.txt").is_none());

        // Plain missing source.
        let status = try_rename_entry(
            &mounts,
            &boot,
            &mut mutable,
            &mut index,
            b"home/ghost.txt",
            b"home/anywhere.txt",
            90,
        );
        assert_eq!(status, StorageStatus::NotFound);
        assert!(!index.stats().1);
    }

    #[test]
    fn read_only_mounts_are_denied() {
        let mounts = seeded_mounts();
        let boot: [EntrySlot; 0] = [];
        let mut mutable = [MutableEntry::empty(); MAX_MUTABLE_ENTRIES];
        insert(&mut mutable, b"rom/locked.txt", StorageEntryKind::File, 4);
        insert(&mut mutable, b"home/note.txt", StorageEntryKind::File, 7);
        let mut index = build_index(&boot, &mutable);

        let status = try_rename_entry(
            &mounts,
            &boot,
            &mut mutable,
            &mut index,
            b"rom/locked.txt",
            b"rom/freed.txt",
            90,
        );
        assert_eq!(status, StorageStatus::Denied);

        let status = try_rename_entry(
            &mounts,
            &boot,
            &mut mutable,
            &mut index,
            b"home/note.txt",
            b"rom/locked2.txt",
            90,
        );
        assert_eq!(status, StorageStatus::Denied);
        assert!(find_mutable_entry(&mutable, b"home/note.txt").is_some());
        assert!(!index.stats().1);
    }

    #[test]
    fn invalid_name_gates_reject() {
        let mounts = seeded_mounts();
        let boot: [EntrySlot; 0] = [];
        let mut mutable = [MutableEntry::empty(); MAX_MUTABLE_ENTRIES];
        insert(&mut mutable, b"home/note.txt", StorageEntryKind::File, 7);
        insert(&mut mutable, b"home/box/", StorageEntryKind::Directory, 0);
        // Corrupt kind/slash pairing to exercise the defense-in-depth gate.
        let odd = mutable.iter_mut().find(|entry| !entry.occupied).unwrap();
        *odd = mutable_entry(b"home/weird/", StorageEntryKind::File, 1);
        let mut index = build_index(&boot, &mutable);

        // Destination kind disagrees with the source kind (slash mismatch).
        let status = try_rename_entry(
            &mounts,
            &boot,
            &mut mutable,
            &mut index,
            b"home/note.txt",
            b"home/dirname/",
            90,
        );
        assert_eq!(status, StorageStatus::InvalidPath);

        // A directory may not move into its own subtree.
        let status = try_rename_entry(
            &mounts,
            &boot,
            &mut mutable,
            &mut index,
            b"home/box/",
            b"home/box/inner/",
            90,
        );
        assert_eq!(status, StorageStatus::InvalidPath);

        // Missing destination parent.
        let status = try_rename_entry(
            &mounts,
            &boot,
            &mut mutable,
            &mut index,
            b"home/note.txt",
            b"home/nope/x.txt",
            90,
        );
        assert_eq!(status, StorageStatus::NotFound);

        // Live entry kind disagrees with the requested path form.
        let status = try_rename_entry(
            &mounts,
            &boot,
            &mut mutable,
            &mut index,
            b"home/weird/",
            b"home/fixed/",
            90,
        );
        assert_eq!(status, StorageStatus::InvalidPath);
    }

    #[test]
    fn subtree_overflow_rejects_atomically() {
        let mounts = seeded_mounts();
        let boot: [EntrySlot; 0] = [];
        let mut mutable = [MutableEntry::empty(); MAX_MUTABLE_ENTRIES];
        insert(&mut mutable, b"home/big/", StorageEntryKind::Directory, 0);
        // Child path exactly MAX_STORAGE_PATH bytes long: a 79-byte suffix
        // under the moved directory.
        let mut child = b"home/big/".to_vec();
        child.resize(MAX_STORAGE_PATH, b'x');
        insert(&mut mutable, &child, StorageEntryKind::File, 8);
        let mut index = build_index(&boot, &mutable);

        // The deep landing zone would push the child past the path limit.
        let status = try_rename_entry(
            &mounts,
            &boot,
            &mut mutable,
            &mut index,
            b"home/big/",
            b"tmp/relocated/",
            90,
        );
        assert_eq!(status, StorageStatus::InvalidPath);
        assert!(find_mutable_entry(&mutable, b"home/big/").is_some());
        assert!(find_mutable_entry(&mutable, &child).is_some());
        assert!(!index.stats().1);
        assert!(
            index
                .snapshot()
                .iter()
                .any(|entry| entry.path() == b"home/big/")
        );
    }

    #[test]
    fn subtree_landing_collision_rejects_atomically() {
        let mounts = seeded_mounts();
        let boot: [EntrySlot; 0] = [];
        let mut mutable = [MutableEntry::empty(); MAX_MUTABLE_ENTRIES];
        insert(&mut mutable, b"home/big/", StorageEntryKind::Directory, 0);
        let mut child = b"home/big/".to_vec();
        child.resize(MAX_STORAGE_PATH, b'x');
        insert(&mut mutable, &child, StorageEntryKind::File, 8);
        // A static entry occupying exactly the child's landing spot under
        // the short destination prefix (same 79-byte suffix).
        let mut landed = b"tmp/r/".to_vec();
        landed.resize(6 + (MAX_STORAGE_PATH - 9), b'x');
        insert(&mut mutable, &landed, StorageEntryKind::File, 1);
        let mut index = build_index(&boot, &mutable);

        let status = try_rename_entry(
            &mounts,
            &boot,
            &mut mutable,
            &mut index,
            b"home/big/",
            b"tmp/r/",
            90,
        );
        assert_eq!(status, StorageStatus::AlreadyExists);
        assert!(find_mutable_entry(&mutable, b"home/big/").is_some());
        assert!(find_mutable_entry(&mutable, &child).is_some());
        assert!(find_mutable_entry(&mutable, &landed).is_some());
        assert!(!index.stats().1);
    }

    #[test]
    fn self_rename_is_a_no_op() {
        let mounts = seeded_mounts();
        let boot: [EntrySlot; 0] = [];
        let mut mutable = [MutableEntry::empty(); MAX_MUTABLE_ENTRIES];
        insert(&mut mutable, b"home/note.txt", StorageEntryKind::File, 7);
        insert(&mut mutable, b"home/box/", StorageEntryKind::Directory, 0);
        let mut index = build_index(&boot, &mutable);
        let before = indexed_paths(&index);

        let status = try_rename_entry(
            &mounts,
            &boot,
            &mut mutable,
            &mut index,
            b"home/note.txt",
            b"home/note.txt",
            95,
        );
        assert_eq!(status, StorageStatus::Ok);
        let status = try_rename_entry(
            &mounts,
            &boot,
            &mut mutable,
            &mut index,
            b"home/box/",
            b"home/box/",
            95,
        );
        assert_eq!(status, StorageStatus::Ok);
        assert_eq!(indexed_paths(&index), before);
        assert!(!index.stats().1);
        assert!(find_mutable_entry(&mutable, b"home/note.txt").is_some());
    }

    fn plan_paths_of(plan: &SearchPlan, snapshot: &[crate::index::IndexEntry]) -> Vec<Vec<u8>> {
        (0..plan.len)
            .map(|at| snapshot[plan.order[at]].path().to_vec())
            .collect()
    }
}
