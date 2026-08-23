use core::fmt::Write;

use rt::{LogEvent, LogSeverity, RawMessage, ServiceId, TerminalTag};
use serviceos_shell_service::{
    SHELL_PROMPT, SHELL_READY_TEXT, ShellOutput, execute_command_with_source, write_output_linef,
};
use serviceos_userspace_runtime as rt;

use crate::{
    logging::emit_terminal_log,
    state::{EscapeState, MAX_HISTORY, MAX_INLINE_BYTES, Session, SessionProfile},
};

pub(crate) fn handle_input_byte(
    bootstrap: rt::Handle,
    session: &mut Session,
    byte: u8,
) -> rt::Result<()> {
    if byte == 0x03 {
        terminal_output_write(session.endpoint, "^C\r\n")?;
        clear_line(session);
        return write_session_prompt(session.endpoint, session);
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
            terminal_output_write(session.endpoint, "\r\n")?;
            let line_len = session.line_len;
            if line_len > 0 {
                append_history(session);
                let line = core::str::from_utf8(&session.line[..line_len])
                    .map_err(|_| rt::Error::InvalidArgument)?;
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    let output = ShellOutput::new(session.endpoint, terminal_output_write);
                    if let Err(error) =
                        execute_command_with_source(bootstrap, ServiceId::Terminal, output, trimmed)
                    {
                        let _ = write_output_linef(
                            output,
                            format_args!(
                                "command failed: {}",
                                serviceos_shell_service::util::error_name(error)
                            ),
                        );
                    }
                }
            }
            clear_line(session);
            write_session_prompt(session.endpoint, session)?;
        }
        0x08 | 0x7f => backspace(session)?,
        0x20..=0x7e => insert_byte(session, byte)?,
        _ => {}
    }
    Ok(())
}

pub(crate) fn initialize_session(
    bootstrap: rt::Handle,
    session: &mut Session,
    next_session_id: &mut u32,
    profile: Option<SessionProfile>,
) -> rt::Result<rt::HandlePair> {
    let pair = rt::channel_create()?;
    session.endpoint = pair.first;
    session.id = *next_session_id;
    *next_session_id = next_session_id.saturating_add(1);
    session.columns = crate::state::DEFAULT_COLS;
    session.rows = crate::state::DEFAULT_ROWS;
    session.line_len = 0;
    session.line_cursor = 0;
    session.history_view = None;
    session.history_stash_len = 0;
    session.escape_state = EscapeState::None;
    session.profile = profile.unwrap_or(SessionProfile::empty());
    session.occupied = true;
    let _ = emit_terminal_log(
        bootstrap,
        LogSeverity::Info,
        LogEvent::TerminalSessionOpened,
        session.id as u64,
        0,
    );
    let _ = terminal_output_write(pair.first, SHELL_READY_TEXT);
    let _ = terminal_output_write(pair.first, "\r\n");
    write_profile_banner(pair.first, &session.profile)?;
    write_session_prompt(pair.first, session)?;
    Ok(pair)
}

/// Echo the session-profile launch metadata (name/program/args/env/cwd) so the
/// operator can see which profile the pane was opened with.
fn write_profile_banner(endpoint: rt::Handle, profile: &SessionProfile) -> rt::Result<()> {
    if profile.name_len > 0 {
        let mut line = rt::FixedLogBuffer::<48>::new();
        let name = core::str::from_utf8(&profile.name[..profile.name_len]).unwrap_or("");
        let _ = write!(&mut line, "[profile {}]", name);
        terminal_output_write(endpoint, core::str::from_utf8(line.as_bytes()).unwrap_or(""))?;
        terminal_output_write(endpoint, "\r\n")?;
    }
    if profile.program_len > 0 {
        let mut line = rt::FixedLogBuffer::<64>::new();
        let program = core::str::from_utf8(&profile.program[..profile.program_len]).unwrap_or("");
        let args = core::str::from_utf8(&profile.args[..profile.args_len]).unwrap_or("");
        let _ = write!(&mut line, "shell: {} {}", program, args);
        terminal_output_write(endpoint, core::str::from_utf8(line.as_bytes()).unwrap_or(""))?;
        terminal_output_write(endpoint, "\r\n")?;
    }
    if profile.env_len > 0 {
        let mut line = rt::FixedLogBuffer::<64>::new();
        let env = core::str::from_utf8(&profile.env[..profile.env_len]).unwrap_or("");
        let _ = write!(&mut line, "env: {}", env);
        terminal_output_write(endpoint, core::str::from_utf8(line.as_bytes()).unwrap_or(""))?;
        terminal_output_write(endpoint, "\r\n")?;
    }
    if profile.cwd_len > 0 {
        let mut line = rt::FixedLogBuffer::<48>::new();
        let cwd = core::str::from_utf8(&profile.cwd[..profile.cwd_len]).unwrap_or("");
        let _ = write!(&mut line, "cwd: {}", cwd);
        terminal_output_write(endpoint, core::str::from_utf8(line.as_bytes()).unwrap_or(""))?;
        terminal_output_write(endpoint, "\r\n")?;
    }
    Ok(())
}

/// Prompt reflects the profile working directory when one is set.
fn write_session_prompt(endpoint: rt::Handle, session: &Session) -> rt::Result<()> {
    let profile = &session.profile;
    if profile.cwd_len == 0 {
        return terminal_output_write(endpoint, SHELL_PROMPT);
    }
    let mut buffer = rt::FixedLogBuffer::<64>::new();
    let cwd = core::str::from_utf8(&profile.cwd[..profile.cwd_len]).unwrap_or("");
    let _ = write!(&mut buffer, "{}{} ", cwd, SHELL_PROMPT);
    terminal_output_write(
        endpoint,
        core::str::from_utf8(buffer.as_bytes()).unwrap_or(SHELL_PROMPT),
    )
}

pub(crate) fn release_session(bootstrap: rt::Handle, session: &mut Session) {
    if !session.occupied {
        return;
    }
    let _ = emit_terminal_log(
        bootstrap,
        LogSeverity::Info,
        LogEvent::TerminalSessionClosed,
        session.id as u64,
        0,
    );
    let _ = rt::channel_send(
        session.endpoint,
        &RawMessage::empty(TerminalTag::SessionClosed as u32),
    );
    let _ = rt::handle_close(session.endpoint);
    *session = Session::empty();
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

pub(crate) fn unpack_bytes(words: &[u64], len: usize, destination: &mut [u8]) -> rt::Result<()> {
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

fn redraw_input_line(session: &Session) -> rt::Result<()> {
    terminal_output_write(session.endpoint, "\r\x1b[2K")?;
    write_session_prompt(session.endpoint, session)?;
    if session.line_len > 0 {
        let text = core::str::from_utf8(&session.line[..session.line_len])
            .map_err(|_| rt::Error::InvalidArgument)?;
        terminal_output_write(session.endpoint, text)?;
    }
    let tail = session.line_len.saturating_sub(session.line_cursor);
    if tail > 0 {
        write_cursor_left(session.endpoint, tail)?;
    }
    Ok(())
}

fn clear_line(session: &mut Session) {
    session.line_len = 0;
    session.line_cursor = 0;
    session.history_view = None;
    session.history_stash_len = 0;
    session.escape_state = EscapeState::None;
}

fn write_cursor_left(endpoint: rt::Handle, count: usize) -> rt::Result<()> {
    if count == 0 {
        return Ok(());
    }
    let mut buffer = rt::FixedLogBuffer::<16>::new();
    let _ = write!(&mut buffer, "\x1b[{}D", count);
    let text = core::str::from_utf8(buffer.as_bytes()).map_err(|_| rt::Error::InvalidArgument)?;
    terminal_output_write(endpoint, text)
}

pub(crate) fn terminal_output_write(endpoint: rt::Handle, text: &str) -> rt::Result<()> {
    let bytes = text.as_bytes();
    let mut offset = 0usize;
    while offset < bytes.len() {
        let end = (offset + MAX_INLINE_BYTES).min(bytes.len());
        let chunk = &bytes[offset..end];
        let mut message = RawMessage::empty(TerminalTag::SessionOutput as u32);
        message.word_count = 1 + pack_bytes(chunk, &mut message.words[1..])?;
        message.words[0] = chunk.len() as u64;
        rt::channel_send(endpoint, &message)?;
        offset = end;
    }
    Ok(())
}
