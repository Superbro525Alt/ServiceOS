use crate::{
    channel_create, channel_receive_blocking, channel_receive_nonblocking, channel_send,
    handle_close, pack_bytes, unpack_bytes, rights, Error, Handle, RawMessage, Result,
    TerminalStatus, TerminalTag, IPC_MAX_WORDS,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalSessionStatusInfo {
    pub session_id: u32,
    pub columns: u32,
    pub rows: u32,
    pub width_pixels: u32,
    pub height_pixels: u32,
}

pub fn terminal_session_open(service_handle: Handle) -> Result<(u32, Handle, u32, u32)> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(TerminalTag::SessionOpenRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(service_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != TerminalTag::SessionOpenReply as u32 || response.word_count < 4 {
        return Err(Error::InvalidArgument);
    }
    match terminal_status_from_word(response.words[0]) {
        TerminalStatus::Ok if response.handle_count > 0 => Ok((
            response.words[1] as u32,
            response.handles[0],
            response.words[2] as u32,
            response.words[3] as u32,
        )),
        status => Err(terminal_status_error(status)),
    }
}

pub fn terminal_session_list(service_handle: Handle, ids: &mut [u32]) -> Result<usize> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(TerminalTag::SessionListRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(service_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != TerminalTag::SessionListReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    match terminal_status_from_word(response.words[0]) {
        TerminalStatus::Ok => {
            let count = response.words[1] as usize;
            if count > ids.len() || response.word_count < (2 + count) as u32 {
                return Err(Error::BufferTooSmall);
            }
            for index in 0..count {
                ids[index] = response.words[2 + index] as u32;
            }
            Ok(count)
        }
        status => Err(terminal_status_error(status)),
    }
}

pub fn terminal_session_status(
    service_handle: Handle,
    session_id: u32,
) -> Result<Option<TerminalSessionStatusInfo>> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(TerminalTag::SessionStatusRequest as u32);
    request.word_count = 1;
    request.words[0] = session_id as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(service_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != TerminalTag::SessionStatusReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match terminal_status_from_word(response.words[0]) {
        TerminalStatus::Ok if response.word_count >= 6 => Ok(Some(TerminalSessionStatusInfo {
            session_id: response.words[1] as u32,
            columns: response.words[2] as u32,
            rows: response.words[3] as u32,
            width_pixels: response.words[4] as u32,
            height_pixels: response.words[5] as u32,
        })),
        TerminalStatus::NotFound => Ok(None),
        status => Err(terminal_status_error(status)),
    }
}

pub fn terminal_session_send_input(session_handle: Handle, bytes: &[u8]) -> Result<()> {
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(1)) * 8;
    if bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }
    let mut request = RawMessage::empty(TerminalTag::SessionInput as u32);
    request.word_count = 1 + pack_bytes(bytes, &mut request.words[1..])?;
    request.words[0] = bytes.len() as u64;
    channel_send(session_handle, &request)
}

pub fn terminal_session_send_text(session_handle: Handle, text: &str) -> Result<()> {
    terminal_session_send_input(session_handle, text.as_bytes())
}

pub fn terminal_session_resize(
    session_handle: Handle,
    columns: u32,
    rows: u32,
    width_pixels: u32,
    height_pixels: u32,
) -> Result<()> {
    let mut request = RawMessage::empty(TerminalTag::SessionResize as u32);
    request.word_count = 4;
    request.words[0] = columns as u64;
    request.words[1] = rows as u64;
    request.words[2] = width_pixels as u64;
    request.words[3] = height_pixels as u64;
    channel_send(session_handle, &request)
}

pub fn terminal_session_close(session_handle: Handle) -> Result<()> {
    let request = RawMessage::empty(TerminalTag::SessionClose as u32);
    channel_send(session_handle, &request)
}

pub fn terminal_session_receive_nonblocking(
    session_handle: Handle,
    buffer: &mut [u8],
) -> Result<Option<usize>> {
    let mut message = RawMessage::empty(0);
    match channel_receive_nonblocking(session_handle, &mut message) {
        Ok(()) if message.tag == TerminalTag::SessionOutput as u32 && message.word_count >= 1 => {
            let len = message.words[0] as usize;
            unpack_bytes(&message.words[1..message.word_count as usize], len, buffer)?;
            Ok(Some(len))
        }
        Ok(()) if message.tag == TerminalTag::SessionClosed as u32 => Err(Error::NotFound),
        Ok(()) => Err(Error::InvalidArgument),
        Err(Error::QueueEmpty) => Ok(None),
        Err(error) => Err(error),
    }
}

fn terminal_status_from_word(value: u64) -> TerminalStatus {
    match value as u32 {
        x if x == TerminalStatus::Busy as u32 => TerminalStatus::Busy,
        x if x == TerminalStatus::NotFound as u32 => TerminalStatus::NotFound,
        x if x == TerminalStatus::Denied as u32 => TerminalStatus::Denied,
        x if x == TerminalStatus::Closed as u32 => TerminalStatus::Closed,
        _ => TerminalStatus::Ok,
    }
}

fn terminal_status_error(status: TerminalStatus) -> Error {
    match status {
        TerminalStatus::Busy => Error::Busy,
        TerminalStatus::NotFound | TerminalStatus::Closed => Error::NotFound,
        TerminalStatus::Denied => Error::PermissionDenied,
        TerminalStatus::Ok => Error::InvalidArgument,
    }
}
