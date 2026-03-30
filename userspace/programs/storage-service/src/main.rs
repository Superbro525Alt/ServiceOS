#![no_std]
#![no_main]

use serviceos_bundle::{
    parse_boot_store_entry, parse_boot_store_header, BootStoreEntryKind, BootStoreEntryRecord,
    BootStoreHeader,
};
use serviceos_userspace_runtime as rt;
use rt::{
    rights, ControlTag, Handle, LifecycleEvent, RawMessage, ServiceId, StorageEntryKind,
    StorageStatus, StorageTag, IPC_MAX_WORDS,
};

const MAX_BOOTSTORE_ENTRIES: usize = 128;
const MAX_BLOB_SESSIONS: usize = 24;
const MAX_DIRECTORY_SESSIONS: usize = 24;
const MAX_MUTABLE_ENTRIES: usize = 128;
const BOOT_ENTRY_BYTES: usize = BootStoreEntryRecord::encoded_len();
const MAX_STORAGE_PATH: usize = serviceos_bundle::BOOT_STORE_PATH_MAX;
const INITIAL_FILE_CAPACITY: usize = 256;

const MUTABLE_ROOT_HOME: &[u8] = b"home/";
const MUTABLE_ROOT_TMP: &[u8] = b"tmp/";
const MUTABLE_ROOT_STATE: &[u8] = b"state/";
const MUTABLE_ROOT_PROJECTS: &[u8] = b"projects/";
const MUTABLE_ROOTS: [&[u8]; 4] = [
    MUTABLE_ROOT_HOME,
    MUTABLE_ROOT_TMP,
    MUTABLE_ROOT_STATE,
    MUTABLE_ROOT_PROJECTS,
];

#[derive(Clone, Copy)]
struct EntrySlot {
    kind: BootStoreEntryKind,
    data_offset: usize,
    data_len: usize,
    path: [u8; MAX_STORAGE_PATH],
    path_len: usize,
}

impl EntrySlot {
    const fn empty() -> Self {
        Self {
            kind: BootStoreEntryKind::Data,
            data_offset: 0,
            data_len: 0,
            path: [0; MAX_STORAGE_PATH],
            path_len: 0,
        }
    }

    fn matches(&self, path: &[u8]) -> bool {
        self.path_len == path.len() && self.path[..self.path_len] == *path
    }

    fn matches_prefix(&self, prefix: &[u8]) -> bool {
        prefix.len() <= self.path_len && self.path[..prefix.len()] == *prefix
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum BlobSource {
    BootStore,
    Mutable,
}

#[derive(Clone, Copy)]
struct BlobSession {
    endpoint: Handle,
    source: BlobSource,
    data_offset: usize,
    data_len: usize,
    data_handle: Handle,
    entry_index: usize,
    writable: bool,
    occupied: bool,
}

impl BlobSession {
    const fn empty() -> Self {
        Self {
            endpoint: rt::INVALID_HANDLE,
            source: BlobSource::BootStore,
            data_offset: 0,
            data_len: 0,
            data_handle: rt::INVALID_HANDLE,
            entry_index: usize::MAX,
            writable: false,
            occupied: false,
        }
    }
}

#[derive(Clone, Copy)]
struct DirectorySession {
    endpoint: Handle,
    path: [u8; MAX_STORAGE_PATH],
    path_len: usize,
    writable: bool,
    occupied: bool,
}

impl DirectorySession {
    const fn empty() -> Self {
        Self {
            endpoint: rt::INVALID_HANDLE,
            path: [0; MAX_STORAGE_PATH],
            path_len: 0,
            writable: false,
            occupied: false,
        }
    }
}

#[derive(Clone, Copy)]
struct MutableEntry {
    kind: StorageEntryKind,
    path: [u8; MAX_STORAGE_PATH],
    path_len: usize,
    data_handle: Handle,
    data_len: usize,
    data_capacity: usize,
    occupied: bool,
}

impl MutableEntry {
    const fn empty() -> Self {
        Self {
            kind: StorageEntryKind::File,
            path: [0; MAX_STORAGE_PATH],
            path_len: 0,
            data_handle: rt::INVALID_HANDLE,
            data_len: 0,
            data_capacity: 0,
            occupied: false,
        }
    }
}

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf501;
    }
    if startup.tag != rt::ControlTag::Startup as u32
        || startup.handle_count < 1
        || startup.word_count < 5
    {
        return 0xf502;
    }

    let bootstore_handle = startup.handles[0];
    let bootstore_len = startup.words[4] as usize;
    let mut header_bytes = [0u8; BootStoreHeader::encoded_len()];
    if rt::memory_read(bootstore_handle, 0, &mut header_bytes).ok() != Some(header_bytes.len()) {
        return 0xf503;
    }
    let header = match parse_boot_store_header(&header_bytes) {
        Ok(header) => header,
        Err(_) => return 0xf504,
    };
    let entry_count = header.entry_count as usize;
    if entry_count > MAX_BOOTSTORE_ENTRIES {
        return 0xf505;
    }

    let mut table_bytes = [0u8; MAX_BOOTSTORE_ENTRIES * BOOT_ENTRY_BYTES];
    let entry_table_len = entry_count * BOOT_ENTRY_BYTES;
    if rt::memory_read(
        bootstore_handle,
        header.entry_table_offset as usize,
        &mut table_bytes[..entry_table_len],
    )
    .ok()
        != Some(entry_table_len)
    {
        return 0xf506;
    }

    let mut entries = [EntrySlot::empty(); MAX_BOOTSTORE_ENTRIES];
    for index in 0..entry_count {
        let start = index * BOOT_ENTRY_BYTES;
        let end = start + BOOT_ENTRY_BYTES;
        let record = match parse_boot_store_entry(&table_bytes[start..end]) {
            Ok(record) => record,
            Err(_) => return 0xf507,
        };
        let Some(kind) = record.kind() else {
            return 0xf508;
        };
        if record.data_offset as usize + record.data_len as usize > bootstore_len {
            let _ = rt::write_logf(
                "storage",
                format_args!(
                    "invalid entry index={} offset={} len={} total={}",
                    index,
                    record.data_offset,
                    record.data_len,
                    bootstore_len,
                ),
            );
            return 0xf509;
        }
        let path_len = record.path_len as usize;
        entries[index].kind = kind;
        entries[index].data_offset = record.data_offset as usize;
        entries[index].data_len = record.data_len as usize;
        entries[index].path_len = path_len;
        entries[index].path[..path_len].copy_from_slice(&record.path[..path_len]);
    }

    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return 0xf50a,
    };
    if rt::register_service(bootstrap, ServiceId::Storage, public.second).is_err() {
        return 0xf50b;
    }
    let _ = rt::handle_close(public.second);
    let _ = rt::write_logf(
        "storage",
        format_args!("mounted boot-store entries={} bytes={}", entry_count, bootstore_len),
    );

    let mut blob_sessions = [BlobSession::empty(); MAX_BLOB_SESSIONS];
    let mut directory_sessions = [DirectorySession::empty(); MAX_DIRECTORY_SESSIONS];
    let mut mutable_entries = [MutableEntry::empty(); MAX_MUTABLE_ENTRIES];

    loop {
        match poll_lifecycle(bootstrap) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(_) => return 0xf50c,
        }

        let mut root_request = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut root_request) {
            Ok(()) => {
                if handle_root_request(
                    &entries[..entry_count],
                    &mut mutable_entries,
                    &mut blob_sessions,
                    &mut directory_sessions,
                    &root_request,
                )
                .is_err()
                {
                    return 0xf50d;
                }
            }
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => return 0xf50e,
        }

        for session in &mut blob_sessions {
            if !session.occupied {
                continue;
            }
            let mut request = RawMessage::empty(0);
            match rt::channel_receive_nonblocking(session.endpoint, &mut request) {
                Ok(()) => {
                    if handle_blob_request(bootstore_handle, &mut mutable_entries, session, &request)
                        .is_err()
                    {
                        return 0xf50f;
                    }
                }
                Err(rt::Error::QueueEmpty) => {}
                Err(_) => release_blob_session(session),
            }
        }

        for session in &mut directory_sessions {
            if !session.occupied {
                continue;
            }
            let mut request = RawMessage::empty(0);
            match rt::channel_receive_nonblocking(session.endpoint, &mut request) {
                Ok(()) => {
                    if handle_directory_request(
                        &entries[..entry_count],
                        &mut mutable_entries,
                        &mut blob_sessions,
                        session,
                        &request,
                    )
                    .is_err()
                    {
                        return 0xf510;
                    }
                }
                Err(rt::Error::QueueEmpty) => {}
                Err(_) => release_directory_session(session),
            }
        }

        if rt::yield_current().is_err() {
            return 0xf511;
        }
    }
}

fn handle_root_request(
    entries: &[EntrySlot],
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
    blob_sessions: &mut [BlobSession; MAX_BLOB_SESSIONS],
    directory_sessions: &mut [DirectorySession; MAX_DIRECTORY_SESSIONS],
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
        _ => Ok(()),
    }
}

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
            session.source = BlobSource::Mutable;
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
    session.source = BlobSource::BootStore;
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

    let exists = path.is_empty()
        || is_mutable_root(path)
        || boot_directory_exists(entries, path)
        || find_mutable_directory(mutable_entries, path).is_some();
    if !exists {
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
    for entry in mutable_entries.iter().filter(|entry| entry.occupied && path_matches_prefix(&entry.path[..entry.path_len], prefix)) {
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

fn handle_directory_request(
    entries: &[EntrySlot],
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
    blob_sessions: &mut [BlobSession; MAX_BLOB_SESSIONS],
    session: &mut DirectorySession,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.tag == StorageTag::CloseRequest as u32 {
        release_directory_session(session);
        return Ok(());
    }

    match message.tag {
        x if x == StorageTag::DirectoryReadRequest as u32 => {
            handle_directory_read_request(entries, mutable_entries, session, message)
        }
        x if x == StorageTag::DirectoryCreateRequest as u32 => {
            handle_directory_create_request(mutable_entries, session, message)
        }
        x if x == StorageTag::DirectoryRemoveRequest as u32 => {
            handle_directory_remove_request(entries, mutable_entries, session, message)
        }
        x if x == StorageTag::DirectoryOpenFileRequest as u32 => {
            handle_directory_open_file_request(entries, mutable_entries, blob_sessions, session, message)
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
    send_status_only(reply_handle, StorageTag::DirectoryCreateReply, StorageStatus::Ok);
    Ok(())
}

fn handle_directory_remove_request(
    entries: &[EntrySlot],
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
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
            release_mutable_entry(&mut mutable_entries[index]);
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
            release_mutable_entry(&mut mutable_entries[index]);
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
    blob_sessions: &mut [BlobSession; MAX_BLOB_SESSIONS],
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
        blob_session.source = BlobSource::Mutable;
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

        let pair = rt::channel_create()?;
        blob_session.endpoint = pair.first;
        blob_session.source = BlobSource::Mutable;
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
    blob_session.source = BlobSource::BootStore;
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

fn handle_blob_request(
    bootstore_handle: Handle,
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
    session: &mut BlobSession,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.tag == StorageTag::CloseRequest as u32 {
        release_blob_session(session);
        return Ok(());
    }

    match message.tag {
        x if x == StorageTag::ReadRequest as u32 => {
            handle_read_request(bootstore_handle, mutable_entries, session, message)
        }
        x if x == StorageTag::WriteRequest as u32 => {
            handle_write_request(mutable_entries, session, message)
        }
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
        BlobSource::BootStore => {
            rt::memory_read(bootstore_handle, session.data_offset + offset, &mut bytes[..read_len])?
        }
        BlobSource::Mutable => {
            let Some(entry) = mutable_entries.get(session.entry_index).filter(|entry| entry.occupied) else {
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
    mutable_entries: &mut [MutableEntry; MAX_MUTABLE_ENTRIES],
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

fn send_blob_open_reply(
    tag: StorageTag,
    reply_handle: Handle,
    status: StorageStatus,
    len: usize,
    handle: Option<Handle>,
) {
    let mut reply = RawMessage::empty(tag as u32);
    reply.word_count = 2;
    reply.words[0] = status as u32 as u64;
    reply.words[1] = len as u64;
    if let Some(handle) = handle {
        reply.handle_count = 1;
        reply.handles[0] = handle;
        reply.handle_rights[0] =
            rights::SEND | rights::RECEIVE | rights::DUPLICATE | rights::TRANSFER;
    }
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
}

fn send_directory_open_reply(reply_handle: Handle, status: StorageStatus, handle: Option<Handle>) {
    let mut reply = RawMessage::empty(StorageTag::DirectoryOpenReply as u32);
    reply.word_count = 1;
    reply.words[0] = status as u32 as u64;
    if let Some(handle) = handle {
        reply.handle_count = 1;
        reply.handles[0] = handle;
        reply.handle_rights[0] =
            rights::SEND | rights::RECEIVE | rights::DUPLICATE | rights::TRANSFER;
    }
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
}

fn send_status_only(reply_handle: Handle, tag: StorageTag, status: StorageStatus) {
    let mut reply = RawMessage::empty(tag as u32);
    reply.word_count = 1;
    reply.words[0] = status as u32 as u64;
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
}

fn find_mutable_entry(
    entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
    path: &[u8],
) -> Option<usize> {
    entries.iter().position(|entry| {
        entry.occupied && entry.path_len == path.len() && entry.path[..entry.path_len] == *path
    })
}

fn find_mutable_directory(
    entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
    path: &[u8],
) -> Option<usize> {
    find_mutable_entry(entries, path)
        .filter(|index| entries[*index].kind == StorageEntryKind::Directory)
}

fn valid_directory_path(path: &[u8]) -> bool {
    path.is_empty() || path.ends_with(b"/")
}

fn is_mutable_root(path: &[u8]) -> bool {
    MUTABLE_ROOTS.iter().any(|root| *root == path)
}

fn is_mutable_path(path: &[u8]) -> bool {
    MUTABLE_ROOTS
        .iter()
        .any(|root| path.len() >= root.len() && path[..root.len()] == **root)
}

fn is_mutable_directory_path(path: &[u8]) -> bool {
    valid_directory_path(path) && (path.is_empty() || is_mutable_root(path) || is_mutable_path(path))
}

fn path_matches_prefix(path: &[u8], prefix: &[u8]) -> bool {
    prefix.len() <= path.len() && path[..prefix.len()] == *prefix
}

fn boot_directory_exists(entries: &[EntrySlot], path: &[u8]) -> bool {
    entries
        .iter()
        .any(|entry| entry.path_len > path.len() && path_matches_prefix(&entry.path[..entry.path_len], path))
}

fn mutable_directory_has_children(
    entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
    path: &[u8],
) -> bool {
    entries
        .iter()
        .any(|entry| entry.occupied && entry.path_len > path.len() && path_matches_prefix(&entry.path[..entry.path_len], path))
}

fn mutable_root_has_materialized_children(
    entries: &[EntrySlot],
    mutable_entries: &[MutableEntry; MAX_MUTABLE_ENTRIES],
    root: &[u8],
) -> bool {
    boot_directory_exists(entries, root)
        || mutable_entries.iter().any(|entry| {
            entry.occupied && entry.path_len >= root.len() && entry.path[..root.len()] == *root
        })
}

fn directory_child_from_path<'a>(
    path: &'a [u8],
    prefix: &[u8],
) -> Option<(&'a [u8], StorageEntryKind)> {
    if !path_matches_prefix(path, prefix) || path.len() == prefix.len() {
        return None;
    }
    let relative = &path[prefix.len()..];
    let Some(component_len) = relative.iter().position(|byte| *byte == b'/') else {
        return Some((path, StorageEntryKind::File));
    };
    if component_len == 0 {
        return None;
    }
    let child_len = prefix.len() + component_len + 1;
    Some((&path[..child_len], StorageEntryKind::Directory))
}

fn compose_child_path(
    parent: &[u8],
    name: &[u8],
    kind: StorageEntryKind,
) -> Option<([u8; MAX_STORAGE_PATH], usize)> {
    if name.is_empty() || name.iter().any(|byte| *byte == b'/') {
        return None;
    }
    let suffix = if kind == StorageEntryKind::Directory { 1 } else { 0 };
    let total_len = parent.len().checked_add(name.len())?.checked_add(suffix)?;
    if total_len > MAX_STORAGE_PATH {
        return None;
    }
    let mut path = [0u8; MAX_STORAGE_PATH];
    path[..parent.len()].copy_from_slice(parent);
    path[parent.len()..parent.len() + name.len()].copy_from_slice(name);
    if suffix == 1 {
        path[total_len - 1] = b'/';
    }
    Some((path, total_len))
}

fn release_mutable_entry(entry: &mut MutableEntry) {
    if entry.data_handle != rt::INVALID_HANDLE {
        let _ = rt::handle_close(entry.data_handle);
    }
    *entry = MutableEntry::empty();
}

fn release_blob_session(session: &mut BlobSession) {
    if session.endpoint != rt::INVALID_HANDLE {
        let _ = rt::handle_close(session.endpoint);
    }
    *session = BlobSession::empty();
}

fn release_directory_session(session: &mut DirectorySession) {
    if session.endpoint != rt::INVALID_HANDLE {
        let _ = rt::handle_close(session.endpoint);
    }
    *session = DirectorySession::empty();
}

fn pack_bytes(source: &[u8], words: &mut [u64]) -> rt::Result<u32> {
    let required = source.len().div_ceil(8);
    if required > words.len() {
        return Err(rt::Error::BufferTooSmall);
    }

    for (index, chunk) in source.chunks(8).enumerate() {
        let mut bytes = [0u8; 8];
        bytes[..chunk.len()].copy_from_slice(chunk);
        words[index] = u64::from_le_bytes(bytes);
    }
    Ok(required as u32)
}

fn unpack_bytes(words: &[u64], len: usize, destination: &mut [u8]) -> rt::Result<()> {
    if len > destination.len() || len > words.len() * 8 {
        return Err(rt::Error::BufferTooSmall);
    }
    let mut copied = 0usize;
    for word in words.iter().copied() {
        if copied >= len {
            break;
        }
        let bytes = word.to_le_bytes();
        let chunk = (len - copied).min(bytes.len());
        destination[copied..copied + chunk].copy_from_slice(&bytes[..chunk]);
        copied += chunk;
    }
    Ok(())
}

fn poll_lifecycle(bootstrap: rt::Handle) -> rt::Result<bool> {
    let mut message = RawMessage::empty(0);
    match rt::channel_receive_nonblocking(bootstrap, &mut message) {
        Ok(()) if message.tag == ControlTag::Lifecycle as u32 && message.word_count > 0 => {
            Ok(matches!(
                lifecycle_event_from_word(message.words[0]),
                LifecycleEvent::Restarting | LifecycleEvent::Stopped
            ))
        }
        Ok(()) => Ok(false),
        Err(rt::Error::QueueEmpty) => Ok(false),
        Err(error) => Err(error),
    }
}

fn lifecycle_event_from_word(value: u64) -> LifecycleEvent {
    match value as u32 {
        x if x == LifecycleEvent::Starting as u32 => LifecycleEvent::Starting,
        x if x == LifecycleEvent::Ready as u32 => LifecycleEvent::Ready,
        x if x == LifecycleEvent::Failed as u32 => LifecycleEvent::Failed,
        x if x == LifecycleEvent::Stopped as u32 => LifecycleEvent::Stopped,
        _ => LifecycleEvent::Restarting,
    }
}
