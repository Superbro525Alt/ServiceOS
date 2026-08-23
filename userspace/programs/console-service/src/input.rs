use serviceos_userspace_runtime as rt;

use crate::session::pack_bytes;
use crate::state::{
    EscapeState, MAX_DISPLAY_BYTES, MAX_HISTORY, MAX_SESSIONS, Session, pop_display_byte,
    push_display_byte, reset_input_state,
};

pub(crate) fn handle_input_byte(
    sessions: &mut [Session; MAX_SESSIONS],
    byte: u8,
) -> rt::Result<()> {
    let Some(session) = sessions
        .iter_mut()
        .find(|session| session.occupied && session.pending_reply != rt::INVALID_HANDLE)
    else {
        return Ok(());
    };

    if byte == 0x03 {
        let _ = rt::debug_console_write(b"^C\r\n");
        reply_with_current_line(session, 0)?;
        return Ok(());
    }

    match session.escape_state {
        EscapeState::Esc => {
            session.escape_state = if byte == b'[' {
                EscapeState::Csi
            } else {
                EscapeState::None
            };
            return Ok(());
        }
        EscapeState::Csi => {
            session.escape_state = EscapeState::None;
            match byte {
                b'A' => history_up(session)?,
                b'B' => history_down(session)?,
                b'C' => move_cursor_right(session)?,
                b'D' => move_cursor_left(session)?,
                _ => {}
            }
            return Ok(());
        }
        EscapeState::None => {}
    }

    match byte {
        0x1b => session.escape_state = EscapeState::Esc,
        b'\r' | b'\n' => {
            let _ = rt::debug_console_write(b"\r\n");
            let reply_len = session.line_len;
            if reply_len > 0 {
                append_history(session);
            }
            reply_with_current_line(session, reply_len)?;
        }
        0x08 | 0x7f => backspace(session)?,
        0x20..=0x7e => insert_byte(session, byte)?,
        _ => {}
    }

    Ok(())
}

fn reply_with_current_line(session: &mut Session, reply_len: usize) -> rt::Result<()> {
    let mut reply = rt::RawMessage::empty(rt::ConsoleTag::SessionReadLineReply as u32);
    reply.word_count = 1;
    reply.words[0] = reply_len as u64;
    reply.word_count += pack_bytes(&session.line[..reply_len], &mut reply.words[1..])?;
    let _ = rt::channel_send(session.pending_reply, &reply);
    let _ = rt::handle_close(session.pending_reply);
    session.pending_reply = rt::INVALID_HANDLE;
    reset_input_state(session);
    Ok(())
}

fn insert_byte(session: &mut Session, byte: u8) -> rt::Result<()> {
    if session.line_len >= session.line.len() {
        return Ok(());
    }
    if session.line_cursor < session.line_len {
        let mut index = session.line_len;
        while index > session.line_cursor {
            session.line[index] = session.line[index - 1];
            index -= 1;
        }
    }
    session.line[session.line_cursor] = byte;
    session.line_len += 1;
    session.line_cursor += 1;
    redraw_input_line(session)
}

fn backspace(session: &mut Session) -> rt::Result<()> {
    if session.line_cursor == 0 || session.line_len == 0 {
        return Ok(());
    }
    let start = session.line_cursor - 1;
    let mut index = start;
    while index + 1 < session.line_len {
        session.line[index] = session.line[index + 1];
        index += 1;
    }
    session.line_len -= 1;
    session.line_cursor -= 1;
    redraw_input_line(session)
}

fn move_cursor_left(session: &mut Session) -> rt::Result<()> {
    if session.line_cursor > 0 {
        session.line_cursor -= 1;
        redraw_input_line(session)?;
    }
    Ok(())
}

fn move_cursor_right(session: &mut Session) -> rt::Result<()> {
    if session.line_cursor < session.line_len {
        session.line_cursor += 1;
        redraw_input_line(session)?;
    }
    Ok(())
}

fn history_up(session: &mut Session) -> rt::Result<()> {
    if session.history_count == 0 {
        return Ok(());
    }
    let next_view = match session.history_view {
        None => {
            session.history_stash[..session.line_len]
                .copy_from_slice(&session.line[..session.line_len]);
            session.history_stash_len = session.line_len;
            session.history_count - 1
        }
        Some(0) => 0,
        Some(index) => index - 1,
    };
    session.history_view = Some(next_view);
    load_history_entry(session, next_view);
    redraw_input_line(session)
}

fn history_down(session: &mut Session) -> rt::Result<()> {
    let Some(current) = session.history_view else {
        return Ok(());
    };
    if current + 1 >= session.history_count {
        session.history_view = None;
        session.line[..session.history_stash_len]
            .copy_from_slice(&session.history_stash[..session.history_stash_len]);
        session.line_len = session.history_stash_len;
        session.line_cursor = session.line_len;
        return redraw_input_line(session);
    }
    let next_view = current + 1;
    session.history_view = Some(next_view);
    load_history_entry(session, next_view);
    redraw_input_line(session)
}

fn append_history(session: &mut Session) {
    if session.line_len == 0 {
        return;
    }
    if session.history_count > 0 {
        let latest_order = session.history_count - 1;
        let latest_slot = history_slot(session, latest_order);
        let latest_len = session.history_lens[latest_slot];
        if latest_len == session.line_len
            && session.history[latest_slot][..latest_len] == session.line[..session.line_len]
        {
            return;
        }
    }

    let slot = session.history_head;
    session.history[slot][..session.line_len].copy_from_slice(&session.line[..session.line_len]);
    session.history_lens[slot] = session.line_len;
    session.history_head = (session.history_head + 1) % MAX_HISTORY;
    if session.history_count < MAX_HISTORY {
        session.history_count += 1;
    }
}

fn load_history_entry(session: &mut Session, order_index: usize) {
    let slot = history_slot(session, order_index);
    let len = session.history_lens[slot];
    session.line[..len].copy_from_slice(&session.history[slot][..len]);
    session.line_len = len;
    session.line_cursor = len;
}

fn history_slot(session: &Session, order_index: usize) -> usize {
    (session.history_head + MAX_HISTORY - session.history_count + order_index) % MAX_HISTORY
}

fn redraw_input_line(session: &mut Session) -> rt::Result<()> {
    rebuild_display(session);
    render_session_line(session)
}

fn rebuild_display(session: &mut Session) {
    let keep = session
        .prompt_len
        .min(session.display_len)
        .min(session.display.len());
    let prompt = session.display;
    let mut next = [0u8; MAX_DISPLAY_BYTES];
    next[..keep].copy_from_slice(&prompt[..keep]);
    let line_copy = session.line_len.min(next.len().saturating_sub(keep));
    next[keep..keep + line_copy].copy_from_slice(&session.line[..line_copy]);
    session.display = next;
    session.display_len = keep + line_copy;
}

pub(crate) fn render_session_line(session: &Session) -> rt::Result<()> {
    rt::debug_console_write(b"\r\x1b[2K")?;
    if session.display_len > 0 {
        rt::debug_console_write(&session.display[..session.display_len])?;
    }
    let end_cursor = session.display_len.saturating_sub(session.prompt_len);
    if end_cursor > session.line_cursor {
        write_cursor_left(end_cursor - session.line_cursor)?;
    }
    Ok(())
}

fn write_cursor_left(count: usize) -> rt::Result<()> {
    if count == 0 {
        return Ok(());
    }
    let mut buffer = [0u8; 16];
    let mut len = 0usize;
    buffer[len] = 0x1b;
    len += 1;
    buffer[len] = b'[';
    len += 1;
    len += write_unsigned_ascii(count, &mut buffer[len..])?;
    buffer[len] = b'D';
    len += 1;
    rt::debug_console_write(&buffer[..len])
}

fn write_unsigned_ascii(mut value: usize, out: &mut [u8]) -> rt::Result<usize> {
    if out.is_empty() {
        return Err(rt::Error::BufferTooSmall);
    }
    if value == 0 {
        out[0] = b'0';
        return Ok(1);
    }
    let mut scratch = [0u8; 20];
    let mut digits = 0usize;
    while value > 0 {
        scratch[digits] = b'0' + (value % 10) as u8;
        digits += 1;
        value /= 10;
    }
    if digits > out.len() {
        return Err(rt::Error::BufferTooSmall);
    }
    for index in 0..digits {
        out[index] = scratch[digits - 1 - index];
    }
    Ok(digits)
}

pub(crate) fn write_session_bytes(session: &mut Session, bytes: &[u8]) -> rt::Result<()> {
    rt::debug_console_write(bytes)?;
    for byte in bytes.iter().copied() {
        match byte {
            b'\r' | b'\n' => {
                session.display_len = 0;
                session.prompt_len = 0;
            }
            0x08 | 0x7f => pop_display_byte(session),
            _ => push_display_byte(session, byte),
        }
    }
    Ok(())
}
