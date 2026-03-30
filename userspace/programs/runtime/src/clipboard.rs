use crate::{
    ClipboardStatus, ClipboardTag, Error, Handle, IPC_MAX_WORDS, RawMessage, Result, channel_create,
    channel_receive_blocking, channel_send, handle_close, pack_bytes, unpack_bytes, rights,
};

pub fn clipboard_write(service_handle: Handle, bytes: &[u8]) -> Result<()> {
    let max_inline_bytes = IPC_MAX_WORDS * 8;
    if bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let reply = channel_create()?;
    let mut request = RawMessage::empty(ClipboardTag::WriteRequest as u32);
    request.word_count = 1 + pack_bytes(bytes, &mut request.words[1..])?;
    request.words[0] = bytes.len() as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(service_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != ClipboardTag::WriteReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match clipboard_status_from_word(response.words[0]) {
        ClipboardStatus::Ok => Ok(()),
        status => Err(clipboard_status_error(status)),
    }
}

pub fn clipboard_read(service_handle: Handle, buffer: &mut [u8]) -> Result<usize> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(ClipboardTag::ReadRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(service_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != ClipboardTag::ReadReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match clipboard_status_from_word(response.words[0]) {
        ClipboardStatus::Ok if response.word_count >= 2 => {
            let len = response.words[1] as usize;
            unpack_bytes(&response.words[2..response.word_count as usize], len, buffer)?;
            Ok(len)
        }
        status => Err(clipboard_status_error(status)),
    }
}

fn clipboard_status_from_word(value: u64) -> ClipboardStatus {
    match value as u32 {
        x if x == ClipboardStatus::NotFound as u32 => ClipboardStatus::NotFound,
        x if x == ClipboardStatus::Denied as u32 => ClipboardStatus::Denied,
        _ => ClipboardStatus::Ok,
    }
}

fn clipboard_status_error(status: ClipboardStatus) -> Error {
    match status {
        ClipboardStatus::NotFound => Error::NotFound,
        ClipboardStatus::Denied => Error::PermissionDenied,
        ClipboardStatus::Ok => Error::InvalidArgument,
    }
}
