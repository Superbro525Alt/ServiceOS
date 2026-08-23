use crate::{
    Error, Handle, IPC_MAX_WORDS, RawMessage, Result, channel_receive_nonblocking, channel_send,
    pack_bytes, unpack_bytes,
};

pub const TEXT_RELAY_TAG: u32 = 1;

pub fn text_relay_write(relay_handle: Handle, text: &str) -> Result<()> {
    let bytes = text.as_bytes();
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(1)) * 8;
    if bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let mut message = RawMessage::empty(TEXT_RELAY_TAG);
    message.word_count = 1 + pack_bytes(bytes, &mut message.words[1..])?;
    message.words[0] = bytes.len() as u64;
    channel_send(relay_handle, &message)
}

pub fn text_relay_try_read(relay_handle: Handle, buffer: &mut [u8]) -> Result<usize> {
    let mut message = RawMessage::empty(0);
    channel_receive_nonblocking(relay_handle, &mut message)?;
    if message.tag != TEXT_RELAY_TAG || message.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    let len = message.words[0] as usize;
    if len > buffer.len() {
        return Err(Error::BufferTooSmall);
    }
    unpack_bytes(&message.words[1..message.word_count as usize], len, buffer)?;
    Ok(len)
}

pub const RUNTIME_OUTPUT_RELAY_TAG: u32 = TEXT_RELAY_TAG;

pub fn runtime_output_relay_write(relay_handle: Handle, text: &str) -> Result<()> {
    text_relay_write(relay_handle, text)
}

pub fn runtime_output_relay_try_read(relay_handle: Handle, buffer: &mut [u8]) -> Result<usize> {
    text_relay_try_read(relay_handle, buffer)
}
