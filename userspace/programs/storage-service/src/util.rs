use serviceos_userspace_runtime as rt;
use rt::{rights, Handle, RawMessage, StorageEntryKind, StorageStatus, StorageTag};

pub(crate) fn send_blob_open_reply(
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

pub(crate) fn send_directory_open_reply(
    reply_handle: Handle,
    status: StorageStatus,
    handle: Option<Handle>,
) {
    send_directory_handle_reply(StorageTag::DirectoryOpenReply, reply_handle, status, handle)
}

pub(crate) fn send_directory_handle_reply(
    tag: StorageTag,
    reply_handle: Handle,
    status: StorageStatus,
    handle: Option<Handle>,
) {
    let mut reply = RawMessage::empty(tag as u32);
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

pub(crate) fn send_traverse_reply(
    reply_handle: Handle,
    status: StorageStatus,
    kind: StorageEntryKind,
    len: usize,
    handle: Option<Handle>,
) {
    let mut reply = RawMessage::empty(StorageTag::DirectoryTraverseReply as u32);
    reply.word_count = 3;
    reply.words[0] = status as u32 as u64;
    reply.words[1] = kind as u32 as u64;
    reply.words[2] = len as u64;
    if let Some(handle) = handle {
        reply.handle_count = 1;
        reply.handles[0] = handle;
        reply.handle_rights[0] =
            rights::SEND | rights::RECEIVE | rights::DUPLICATE | rights::TRANSFER;
    }
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
}

pub(crate) fn send_status_only(reply_handle: Handle, tag: StorageTag, status: StorageStatus) {
    let mut reply = RawMessage::empty(tag as u32);
    reply.word_count = 1;
    reply.words[0] = status as u32 as u64;
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
}

pub(crate) fn pack_bytes(source: &[u8], words: &mut [u64]) -> rt::Result<u32> {
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

pub(crate) fn unpack_bytes(words: &[u64], len: usize, destination: &mut [u8]) -> rt::Result<()> {
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
