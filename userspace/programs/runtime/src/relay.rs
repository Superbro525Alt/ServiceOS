use crate::{
    channel_receive_nonblocking, channel_send, pack_bytes, unpack_bytes, Error, Handle,
    RawMessage, Result, IPC_MAX_WORDS,
};

pub const RUNTIME_OUTPUT_RELAY_TAG: u32 = 1;

pub fn runtime_output_relay_write(relay_handle: Handle, text: &str) -> Result<()> {
    let bytes = text.as_bytes();
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(1)) * 8;
    if bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let mut message = RawMessage::empty(RUNTIME_OUTPUT_RELAY_TAG);
    message.word_count = 1 + pack_bytes(bytes, &mut message.words[1..])?;
    message.words[0] = bytes.len() as u64;
    channel_send(relay_handle, &message)
}

pub fn runtime_output_relay_try_read(relay_handle: Handle, buffer: &mut [u8]) -> Result<usize> {
    let mut message = RawMessage::empty(0);
    channel_receive_nonblocking(relay_handle, &mut message)?;
    if message.tag != RUNTIME_OUTPUT_RELAY_TAG || message.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    let len = message.words[0] as usize;
    if len > buffer.len() {
        return Err(Error::BufferTooSmall);
    }
    unpack_bytes(&message.words[1..message.word_count as usize], len, buffer)?;
    Ok(len)
}
