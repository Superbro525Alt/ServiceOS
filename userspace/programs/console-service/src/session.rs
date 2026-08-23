use rt::{ConsoleTag, RawMessage};
use serviceos_userspace_runtime as rt;

use crate::input::write_session_bytes;
use crate::state::{MAX_SESSIONS, Session, begin_input_session, reset_input_state};

pub(crate) fn handle_session_message(
    session: &mut Session,
    message: &RawMessage,
) -> rt::Result<()> {
    match message.tag {
        x if x == ConsoleTag::SessionWriteText as u32 => {
            if message.word_count < 1 {
                return Ok(());
            }
            let text_len = message.words[0] as usize;
            let mut buffer = [0u8; rt::IPC_MAX_WORDS * 8];
            let payload_words = message.word_count as usize;
            unpack_bytes(&message.words[1..payload_words], text_len, &mut buffer)?;
            write_session_bytes(session, &buffer[..text_len])?;
        }
        x if x == ConsoleTag::SessionReadLineRequest as u32 => {
            if message.handle_count < 1 {
                return Ok(());
            }
            if session.pending_reply != rt::INVALID_HANDLE {
                let _ = rt::handle_close(message.handles[0]);
                return Ok(());
            }
            session.pending_reply = message.handles[0];
            begin_input_session(session);
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn handle_session_open(
    sessions: &mut [Session; MAX_SESSIONS],
    reply_handle: rt::Handle,
) -> rt::Result<()> {
    let mut reply = RawMessage::empty(ConsoleTag::SessionOpenReply as u32);

    if let Some(session) = sessions.iter_mut().find(|session| !session.occupied) {
        let pair = rt::channel_create()?;
        session.endpoint = pair.first;
        reset_input_state(session);
        session.occupied = true;

        reply.handle_count = 1;
        reply.handles[0] = pair.second;
        reply.handle_rights[0] =
            rt::rights::SEND | rt::rights::RECEIVE | rt::rights::DUPLICATE | rt::rights::TRANSFER;
        let _ = rt::channel_send(reply_handle, &reply);
        let _ = rt::handle_close(pair.second);
    } else {
        let _ = rt::channel_send(reply_handle, &reply);
    }

    let _ = rt::handle_close(reply_handle);
    Ok(())
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
