use crate::{
    ClipboardHistoryEntry, ClipboardStatus, ClipboardTag, Error, Handle, IPC_MAX_WORDS, RawMessage,
    Result, channel_call, pack_bytes, unpack_bytes,
};

pub fn clipboard_write(service_handle: Handle, bytes: &[u8]) -> Result<()> {
    let max_inline_bytes = IPC_MAX_WORDS * 8;
    if bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let mut request = RawMessage::empty(ClipboardTag::WriteRequest as u32);
    request.word_count = 1 + pack_bytes(bytes, &mut request.words[1..])?;
    request.words[0] = bytes.len() as u64;
    let response = channel_call(service_handle, &mut request)?;
    if response.tag != ClipboardTag::WriteReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match clipboard_status_from_word(response.words[0]) {
        ClipboardStatus::Ok => Ok(()),
        status => Err(clipboard_status_error(status)),
    }
}

pub fn clipboard_read(service_handle: Handle, buffer: &mut [u8]) -> Result<usize> {
    let mut request = RawMessage::empty(ClipboardTag::ReadRequest as u32);
    let response = channel_call(service_handle, &mut request)?;
    if response.tag != ClipboardTag::ReadReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match clipboard_status_from_word(response.words[0]) {
        ClipboardStatus::Ok if response.word_count >= 2 => {
            let len = response.words[1] as usize;
            unpack_bytes(
                &response.words[2..response.word_count as usize],
                len,
                buffer,
            )?;
            Ok(len)
        }
        status => Err(clipboard_status_error(status)),
    }
}

pub fn clipboard_history_entry(
    service_handle: Handle,
    index: u32,
) -> Result<ClipboardHistoryEntry> {
    let mut request = RawMessage::empty(ClipboardTag::HistoryRequest as u32);
    request.word_count = 1;
    request.words[0] = index as u64;
    let response = channel_call(service_handle, &mut request)?;
    if response.tag != ClipboardTag::HistoryReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match clipboard_status_from_word(response.words[0]) {
        ClipboardStatus::Ok => {
            if response.word_count < 4 {
                return Err(Error::InvalidArgument);
            }
            let len = response.words[3] as usize;
            let mut bytes = [0u8; 64];
            unpack_bytes(
                &response.words[4..response.word_count as usize],
                len,
                &mut bytes,
            )?;
            Ok(ClipboardHistoryEntry {
                index: response.words[1] as u32,
                active: response.words[2] != 0,
                len: len as u32,
                bytes,
            })
        }
        status => Err(clipboard_status_error(status)),
    }
}

pub fn clipboard_activate(service_handle: Handle, index: u32) -> Result<()> {
    let mut request = RawMessage::empty(ClipboardTag::ActivateRequest as u32);
    request.word_count = 1;
    request.words[0] = index as u64;
    let response = channel_call(service_handle, &mut request)?;
    if response.tag != ClipboardTag::ActivateReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match clipboard_status_from_word(response.words[0]) {
        ClipboardStatus::Ok => Ok(()),
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
