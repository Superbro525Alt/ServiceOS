#![no_std]
#![no_main]

use serviceos_bundle::{
    parse_boot_store_entry, parse_boot_store_header, BootStoreEntryKind, BootStoreEntryRecord,
    BootStoreHeader,
};
use serviceos_userspace_runtime as rt;
use rt::{rights, Handle, RawMessage, ServiceId, StorageStatus, StorageTag, IPC_MAX_WORDS};

const MAX_BOOTSTORE_ENTRIES: usize = 32;
const MAX_BLOB_SESSIONS: usize = 24;
const BOOT_ENTRY_BYTES: usize = BootStoreEntryRecord::encoded_len();

#[derive(Clone, Copy)]
struct EntrySlot {
    kind: BootStoreEntryKind,
    data_offset: usize,
    data_len: usize,
    path: [u8; serviceos_bundle::BOOT_STORE_PATH_MAX],
    path_len: usize,
}

impl EntrySlot {
    const fn empty() -> Self {
        Self {
            kind: BootStoreEntryKind::Data,
            data_offset: 0,
            data_len: 0,
            path: [0; serviceos_bundle::BOOT_STORE_PATH_MAX],
            path_len: 0,
        }
    }

    fn matches(&self, path: &[u8]) -> bool {
        self.path_len == path.len() && self.path[..self.path_len] == *path
    }
}

#[derive(Clone, Copy)]
struct BlobSession {
    endpoint: Handle,
    data_offset: usize,
    data_len: usize,
    occupied: bool,
}

impl BlobSession {
    const fn empty() -> Self {
        Self {
            endpoint: rt::INVALID_HANDLE,
            data_offset: 0,
            data_len: 0,
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
    if startup.tag != rt::ControlTag::Startup as u32 || startup.handle_count < 1 || startup.word_count < 1 {
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

    let mut sessions = [BlobSession::empty(); MAX_BLOB_SESSIONS];
    loop {
        let mut root_request = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut root_request) {
            Ok(()) => {
                if handle_open_request(&entries[..entry_count], &mut sessions, public.first, &root_request).is_err() {
                    return 0xf50c;
                }
            }
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => return 0xf50d,
        }

        for session in &mut sessions {
            if !session.occupied {
                continue;
            }
            let mut request = RawMessage::empty(0);
            match rt::channel_receive_nonblocking(session.endpoint, &mut request) {
                Ok(()) => {
                    if handle_read_request(bootstore_handle, session, &request).is_err() {
                        return 0xf50e;
                    }
                }
                Err(rt::Error::QueueEmpty) => {}
                Err(_) => return 0xf50f,
            }
        }

        if rt::yield_current().is_err() {
            return 0xf510;
        }
    }
}

fn handle_open_request(
    entries: &[EntrySlot],
    sessions: &mut [BlobSession; MAX_BLOB_SESSIONS],
    _public: Handle,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.tag != StorageTag::OpenRequest as u32 || message.word_count < 1 || message.handle_count < 1 {
        return Ok(());
    }

    let path_len = message.words[0] as usize;
    let mut path = [0u8; serviceos_bundle::BOOT_STORE_PATH_MAX];
    if unpack_bytes(&message.words[1..message.word_count as usize], path_len, &mut path).is_err() {
        return Ok(());
    }
    let reply_handle = message.handles[0];

    let Some(entry) = entries.iter().find(|entry| entry.matches(&path[..path_len])) else {
        let mut reply = RawMessage::empty(StorageTag::OpenReply as u32);
        reply.word_count = 2;
        reply.words[0] = StorageStatus::NotFound as u32 as u64;
        reply.words[1] = 0;
        let _ = rt::channel_send(reply_handle, &reply);
        let _ = rt::handle_close(reply_handle);
        return Ok(());
    };

    let Some(session) = sessions.iter_mut().find(|session| !session.occupied) else {
        let _ = rt::handle_close(reply_handle);
        return Err(rt::Error::CapacityExceeded);
    };
    let pair = rt::channel_create()?;
    session.endpoint = pair.first;
    session.data_offset = entry.data_offset;
    session.data_len = entry.data_len;
    session.occupied = true;

    let mut reply = RawMessage::empty(StorageTag::OpenReply as u32);
    reply.word_count = 2;
    reply.words[0] = StorageStatus::Ok as u32 as u64;
    reply.words[1] = entry.data_len as u64;
    reply.handle_count = 1;
    reply.handles[0] = pair.second;
    reply.handle_rights[0] = rights::SEND | rights::RECEIVE | rights::DUPLICATE | rights::TRANSFER;
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    let _ = rt::handle_close(pair.second);
    Ok(())
}

fn handle_read_request(
    bootstore_handle: Handle,
    session: &mut BlobSession,
    message: &RawMessage,
) -> rt::Result<()> {
    if message.tag != StorageTag::ReadRequest as u32 || message.word_count < 2 || message.handle_count < 1 {
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
    let copied = rt::memory_read(
        bootstore_handle,
        session.data_offset + offset,
        &mut bytes[..read_len],
    )?;

    reply.words[0] = StorageStatus::Ok as u32 as u64;
    reply.words[2] = copied as u64;
    reply.word_count += pack_bytes(&bytes[..copied], &mut reply.words[3..])?;
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
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
