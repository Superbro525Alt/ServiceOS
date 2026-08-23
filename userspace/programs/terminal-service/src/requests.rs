use rt::{RawMessage, TerminalStatus, TerminalTag};
use serviceos_userspace_runtime as rt;

use crate::{
    session::{handle_input_byte, initialize_session, release_session, unpack_bytes},
    state::{
        MAX_INLINE_BYTES, MAX_SESSIONS, Session, SessionProfile, PROFILE_WIRE_LEN,
    },
};

pub(crate) fn handle_public_request(
    bootstrap: rt::Handle,
    sessions: &mut [Session; MAX_SESSIONS],
    next_session_id: &mut u32,
    request: &RawMessage,
) -> rt::Result<()> {
    match request.tag {
        x if x == TerminalTag::SessionOpenRequest as u32 => {
            if request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(TerminalTag::SessionOpenReply as u32);
            reply.word_count = 4;
            reply.words[0] = TerminalStatus::Busy as u32 as u64;
            // Optional session-profile payload: words[0] = byte length, then
            // packed wire bytes. Older clients send no words.
            let profile = if request.word_count >= 1 {
                let len = (request.words[0] as usize).min(PROFILE_WIRE_LEN);
                let mut wire = [0u8; PROFILE_WIRE_LEN];
                match unpack_bytes(&request.words[1..request.word_count as usize], len, &mut wire)
                {
                    Ok(()) => SessionProfile::from_wire(&wire),
                    Err(_) => None,
                }
            } else {
                None
            };
            if let Some(session) = sessions.iter_mut().find(|session| !session.occupied) {
                let pair = initialize_session(bootstrap, session, next_session_id, profile)?;

                reply.words[0] = TerminalStatus::Ok as u32 as u64;
                reply.words[1] = session.id as u64;
                reply.words[2] = session.columns as u64;
                reply.words[3] = session.rows as u64;
                reply.handle_count = 1;
                reply.handles[0] = pair.second;
                reply.handle_rights[0] = rt::rights::SEND
                    | rt::rights::RECEIVE
                    | rt::rights::DUPLICATE
                    | rt::rights::TRANSFER;
                let _ = rt::channel_send(reply_handle, &reply);
                let _ = rt::handle_close(pair.second);
            } else {
                let _ = rt::channel_send(reply_handle, &reply);
            }
            let _ = rt::handle_close(reply_handle);
        }
        x if x == TerminalTag::SessionListRequest as u32 => {
            if request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(TerminalTag::SessionListReply as u32);
            reply.word_count = 2;
            reply.words[0] = TerminalStatus::Ok as u32 as u64;
            let mut count = 0usize;
            for session in sessions.iter().filter(|session| session.occupied) {
                if 2 + count >= rt::IPC_MAX_WORDS as usize {
                    break;
                }
                reply.words[2 + count] = session.id as u64;
                count += 1;
            }
            reply.word_count = 2 + count as u32;
            reply.words[1] = count as u64;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == TerminalTag::SessionStatusRequest as u32 => {
            if request.handle_count < 1 || request.word_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let session_id = request.words[0] as u32;
            let mut reply = RawMessage::empty(TerminalTag::SessionStatusReply as u32);
            reply.word_count = 6;
            match sessions
                .iter()
                .find(|session| session.occupied && session.id == session_id)
            {
                Some(session) => {
                    reply.words[0] = TerminalStatus::Ok as u32 as u64;
                    reply.words[1] = session.id as u64;
                    reply.words[2] = session.columns as u64;
                    reply.words[3] = session.rows as u64;
                    reply.words[4] = session.width_pixels as u64;
                    reply.words[5] = session.height_pixels as u64;
                }
                None => {
                    reply.word_count = 1;
                    reply.words[0] = TerminalStatus::NotFound as u32 as u64;
                }
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        _ => {}
    }

    Ok(())
}

pub(crate) fn handle_session_message(
    bootstrap: rt::Handle,
    session: &mut Session,
    message: &RawMessage,
) -> rt::Result<()> {
    match message.tag {
        x if x == TerminalTag::SessionInput as u32 => {
            if message.word_count < 1 {
                return Ok(());
            }
            let len = message.words[0] as usize;
            let mut buffer = [0u8; MAX_INLINE_BYTES];
            unpack_bytes(
                &message.words[1..message.word_count as usize],
                len,
                &mut buffer,
            )?;
            for byte in buffer[..len].iter().copied() {
                handle_input_byte(bootstrap, session, byte)?;
            }
        }
        x if x == TerminalTag::SessionResize as u32 => {
            if message.word_count >= 4 {
                session.columns = message.words[0] as u32;
                session.rows = message.words[1] as u32;
                session.width_pixels = message.words[2] as u32;
                session.height_pixels = message.words[3] as u32;
            }
        }
        x if x == TerminalTag::SessionClose as u32 => {
            release_session(bootstrap, session);
        }
        _ => {}
    }
    Ok(())
}
