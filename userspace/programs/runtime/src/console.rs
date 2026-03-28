use crate::{
    channel_create, channel_receive_blocking, channel_send, handle_close, pack_bytes, rights,
    ConsoleTag, Error, Handle, RawMessage, Result, IPC_MAX_WORDS,
};

pub fn console_write_record(
    console_handle: Handle,
    source: crate::ServiceId,
    severity: crate::LogSeverity,
    domain: crate::LogDomain,
    event: crate::LogEvent,
    arg0: u64,
    arg1: u64,
    sequence: u64,
) -> Result<()> {
    let mut message = RawMessage::empty(ConsoleTag::WriteRecord as u32);
    message.word_count = 7;
    message.words[0] = source as u32 as u64;
    message.words[1] = severity as u32 as u64;
    message.words[2] = domain as u32 as u64;
    message.words[3] = event as u32 as u64;
    message.words[4] = arg0;
    message.words[5] = arg1;
    message.words[6] = sequence;
    channel_send(console_handle, &message)
}

pub fn console_session_open(console_handle: Handle) -> Result<Handle> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(ConsoleTag::SessionOpenRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(console_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != ConsoleTag::SessionOpenReply as u32 || response.handle_count < 1 {
        return Err(Error::Busy);
    }
    Ok(response.handles[0])
}

pub fn console_session_write(session_handle: Handle, text: &str) -> Result<()> {
    let text_bytes = text.as_bytes();
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(1)) * 8;
    if text_bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }
    let mut message = RawMessage::empty(ConsoleTag::SessionWriteText as u32);
    message.word_count = 1 + pack_bytes(text_bytes, &mut message.words[1..])?;
    message.words[0] = text_bytes.len() as u64;
    channel_send(session_handle, &message)
}

pub fn console_session_read_line(session_handle: Handle, buffer: &mut [u8]) -> Result<usize> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(ConsoleTag::SessionReadLineRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(session_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != ConsoleTag::SessionReadLineReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    let len = response.words[0] as usize;
    crate::unpack_bytes(&response.words[1..response.word_count as usize], len, buffer)?;
    Ok(len)
}
