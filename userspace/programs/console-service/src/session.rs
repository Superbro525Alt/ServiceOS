use rt::{ConsoleTag, RawMessage};
use serviceos_userspace_runtime as rt;

use crate::grid;
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
            let payload = &buffer[..text_len];
            // Alternate-screen style markers ride the existing write contract:
            // they toggle the client's console-grid subscription instead of
            // echoing to the serial surface.
            if grid::is_subscribe(payload) {
                session.grid_sub = true;
                send_grid_frame(session);
                return Ok(());
            }
            if grid::is_unsubscribe(payload) {
                session.grid_sub = false;
                return Ok(());
            }
            write_session_bytes(session, payload)?;
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

    // Handoff first: a detached row keeps its history ring, so an operator
    // reattaching continues the previous operator session instead of minting
    // a fresh one (mirrors terminal-service detach/reattach semantics).
    let slot = select_session_slot(sessions);

    let Some(session) = slot.map(|index| &mut sessions[index]) else {
        let _ = rt::channel_send(reply_handle, &reply);
        let _ = rt::handle_close(reply_handle);
        return Ok(());
    };

    let pair = match rt::channel_create() {
        Ok(pair) => pair,
        Err(error) => {
            *session = Session::empty();
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
            return Err(error);
        }
    };
    // Retained history survives: only transient input/display state resets.
    session.endpoint = pair.first;
    reset_input_state(session);
    session.grid_sub = false;
    session.detached = false;
    session.occupied = true;

    reply.handle_count = 1;
    reply.handles[0] = pair.second;
    reply.handle_rights[0] =
        rt::rights::SEND | rt::rights::RECEIVE | rt::rights::DUPLICATE | rt::rights::TRANSFER;
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(pair.second);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

/// Push one formatted console line to every attached subscriber. Payload uses
/// the existing `SessionWriteText` wire shape (len word + packed bytes); only
/// the delivery direction is new.
pub(crate) fn broadcast_grid_line(sessions: &mut [Session; MAX_SESSIONS], line: &[u8]) {
    for session in sessions.iter_mut() {
        if !session.occupied || session.detached || !session.grid_sub {
            continue;
        }
        let mut message = RawMessage::empty(ConsoleTag::SessionWriteText as u32);
        let capped = line.len().min(MAX_BROADCAST_LINE - 2);
        message.words[0] = (capped + 2) as u64;
        // Pack the line plus CRLF into the remaining words.
        let mut bytes = [0u8; MAX_BROADCAST_LINE];
        bytes[..capped].copy_from_slice(&line[..capped]);
        bytes[capped] = b'\r';
        bytes[capped + 1] = b'\n';
        if let Ok(words) = pack_bytes(&bytes[..capped + 2], &mut message.words[1..]) {
            message.word_count = 1 + words;
            let _ = rt::channel_send(session.endpoint, &message);
        }
    }
}

/// Send the full retained grid frame to one subscribed session, chunked into
/// existing write-shaped messages.
pub(crate) fn send_grid_frame(session: &mut Session) {
    let mut frame = [0u8; grid::FRAME_RESET.len() + grid::GRID_ROWS * (grid::GRID_COLS + 2)];
    let len = grid::snapshot_frame(&mut frame);
    let mut offset = 0usize;
    while offset < len {
        let end = (offset + MAX_BROADCAST_LINE).min(len);
        let chunk = &frame[offset..end];
        let mut message = RawMessage::empty(ConsoleTag::SessionWriteText as u32);
        message.words[0] = chunk.len() as u64;
        match pack_bytes(chunk, &mut message.words[1..]) {
            Ok(words) => {
                message.word_count = 1 + words;
                let _ = rt::channel_send(session.endpoint, &message);
            }
            Err(_) => break,
        }
        offset = end;
    }
}

/// Largest payload that fits one message: length word consumes one slot and
/// each word carries 8 bytes.
pub(crate) const MAX_BROADCAST_LINE: usize = (rt::IPC_MAX_WORDS - 1) * 8;

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

/// Slot selection for SessionOpen: adopt a detached row first so retained
/// history survives handoff, then any free row; `None` when all are live.
pub(crate) fn select_session_slot(sessions: &[Session; MAX_SESSIONS]) -> Option<usize> {
    sessions
        .iter()
        .position(|session| session.occupied && session.detached)
        .or_else(|| sessions.iter().position(|session| !session.occupied))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_roundtrips_bytes_through_words() {
        let source = b"grid\x00line!";
        let mut words = [0u64; 4];
        let count = pack_bytes(source, &mut words).expect("fits");
        assert_eq!(count as usize, source.len().div_ceil(8));
        let mut out = [0u8; 32];
        unpack_bytes(&words[..count as usize], source.len(), &mut out).expect("roundtrip");
        assert_eq!(&out[..source.len()], source);
    }

    #[test]
    fn pack_rejects_undersized_word_slice() {
        let mut words = [0u64; 1];
        assert_eq!(
            pack_bytes(&[0u8; 16], &mut words),
            Err(rt::Error::BufferTooSmall)
        );
    }

    #[test]
    fn slot_selection_prefers_detached_then_free() {
        let mut sessions = [Session::empty(); MAX_SESSIONS];
        // Nothing available yet? A free slot always exists while empty.
        assert_eq!(select_session_slot(&sessions), Some(0));
        sessions[0].occupied = true;
        assert_eq!(select_session_slot(&sessions), Some(1));
        // Full and live: nothing to hand out.
        sessions[1].occupied = true;
        assert_eq!(select_session_slot(&sessions), None);
        // Detached rows win over free rows regardless of position.
        sessions[0].detached = true;
        sessions[1].detached = false;
        sessions[1].occupied = false;
        assert_eq!(select_session_slot(&sessions), Some(0));
    }
}
