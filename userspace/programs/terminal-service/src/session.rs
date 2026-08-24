use core::fmt::Write;

use rt::{LogEvent, LogSeverity, RawMessage, ServiceId, TerminalTag};
use serviceos_shell_service::{
    SHELL_PROMPT, SHELL_READY_TEXT, ShellOutput, execute_command_with_source, write_output_linef,
};
use serviceos_userspace_runtime as rt;

use crate::{
    logging::emit_terminal_log,
    state::{
        EscapeState, MAX_HISTORY, MAX_INLINE_BYTES, MAX_LINE_BYTES, SCROLLBACK_BYTES, Session,
        SessionProfile,
    },
};

/// Rights carried by handles transferred to attaching clients.
const ATTACH_RIGHTS: u64 =
    rt::rights::SEND | rt::rights::RECEIVE | rt::rights::DUPLICATE | rt::rights::TRANSFER;

/// Shell-command output recorder: set while a session executes a command so
/// the routed writer can mirror output into that session's scrollback ring.
/// The service is single-threaded; the pointer is cleared on every exit path.
static mut COMMAND_OUTPUT_SESSION: *mut Session = core::ptr::null_mut();

fn recording_output_write(endpoint: rt::Handle, text: &str) -> rt::Result<()> {
    let result = terminal_output_write(endpoint, text);
    // SAFETY: single-threaded service; the target is only set while a
    // command runs on that same session and cleared immediately after.
    let target = unsafe { COMMAND_OUTPUT_SESSION };
    if !target.is_null() {
        unsafe {
            (*target).scrollback.record(text.as_bytes());
        }
    }
    result
}

/// Send text to a session's client without touching retained state.
pub(crate) fn terminal_output_write(endpoint: rt::Handle, text: &str) -> rt::Result<()> {
    output_bytes(endpoint, text.as_bytes())
}

pub(crate) fn output_bytes(endpoint: rt::Handle, bytes: &[u8]) -> rt::Result<()> {
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

/// Record output into the session scrollback, then stream it to the client.
pub(crate) fn emit_output(session: &mut Session, text: &str) -> rt::Result<()> {
    session.scrollback.record(text.as_bytes());
    terminal_output_write(session.endpoint, text)
}

fn emit_output_line(session: &mut Session, bytes: &[u8]) -> rt::Result<()> {
    session.scrollback.record(bytes);
    output_bytes(session.endpoint, bytes)
}

pub(crate) fn handle_input_byte(
    bootstrap: rt::Handle,
    session: &mut Session,
    byte: u8,
) -> rt::Result<()> {
    if byte == 0x03 {
        emit_output(session, "^C\r\n")?;
        clear_line(session);
        return write_session_prompt(session);
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
            emit_output(session, "\r\n")?;
            let line_len = session.line_len;
            if line_len > 0 {
                append_history(session);
                // Copy the line out so the command can run while the session
                // pointer is published for output recording.
                let mut line_bytes = [0u8; MAX_LINE_BYTES];
                line_bytes[..line_len].copy_from_slice(&session.line[..line_len]);
                let text = core::str::from_utf8(&line_bytes[..line_len])
                    .map_err(|_| rt::Error::InvalidArgument)?;
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    let output = ShellOutput::new(session.endpoint, recording_output_write);
                    // SAFETY: single-threaded; cleared below before session is
                    // touched again.
                    unsafe {
                        COMMAND_OUTPUT_SESSION = session as *mut Session;
                    }
                    let result = execute_command_with_source(
                        bootstrap,
                        ServiceId::Terminal,
                        output,
                        trimmed,
                    );
                    if let Err(error) = result {
                        let _ = write_output_linef(
                            output,
                            format_args!(
                                "command failed: {}",
                                serviceos_shell_service::util::error_name(error)
                            ),
                        );
                    }
                    unsafe {
                        COMMAND_OUTPUT_SESSION = core::ptr::null_mut();
                    }
                }
            }
            clear_line(session);
            write_session_prompt(session)?;
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
    session.scrollback.clear();
    session.bookmarks.clear();
    session.attached = true;
    // Retained duplicate used to mint client handles on future reattaches.
    match rt::handle_duplicate(pair.second, ATTACH_RIGHTS) {
        Ok(spare) => session.spare_endpoint = spare,
        Err(error) => {
            *session = Session::empty();
            let _ = rt::handle_close(pair.first);
            return Err(error);
        }
    }
    session.occupied = true;
    let _ = emit_terminal_log(
        bootstrap,
        LogSeverity::Info,
        LogEvent::TerminalSessionOpened,
        session.id as u64,
        0,
    );
    emit_output(session, SHELL_READY_TEXT)?;
    emit_output(session, "\r\n")?;
    write_profile_banner(session)?;
    write_session_prompt(session)?;
    Ok(pair)
}

/// Echo the session-profile launch metadata (name/program/args/env/cwd) so the
/// operator can see which profile the pane was opened with.
fn write_profile_banner(session: &mut Session) -> rt::Result<()> {
    let profile = session.profile;
    if profile.name_len > 0 {
        let mut line = rt::FixedLogBuffer::<48>::new();
        let name = core::str::from_utf8(&profile.name[..profile.name_len]).unwrap_or("");
        let _ = write!(&mut line, "[profile {}]", name);
        emit_output_line(session, line.as_bytes())?;
        emit_output(session, "\r\n")?;
    }
    if profile.program_len > 0 {
        let mut line = rt::FixedLogBuffer::<64>::new();
        let program = core::str::from_utf8(&profile.program[..profile.program_len]).unwrap_or("");
        let args = core::str::from_utf8(&profile.args[..profile.args_len]).unwrap_or("");
        let _ = write!(&mut line, "shell: {} {}", program, args);
        emit_output_line(session, line.as_bytes())?;
        emit_output(session, "\r\n")?;
    }
    if profile.env_len > 0 {
        let mut line = rt::FixedLogBuffer::<64>::new();
        let env = core::str::from_utf8(&profile.env[..profile.env_len]).unwrap_or("");
        let _ = write!(&mut line, "env: {}", env);
        emit_output_line(session, line.as_bytes())?;
        emit_output(session, "\r\n")?;
    }
    if profile.cwd_len > 0 {
        let mut line = rt::FixedLogBuffer::<48>::new();
        let cwd = core::str::from_utf8(&profile.cwd[..profile.cwd_len]).unwrap_or("");
        let _ = write!(&mut line, "cwd: {}", cwd);
        emit_output_line(session, line.as_bytes())?;
        emit_output(session, "\r\n")?;
    }
    Ok(())
}

/// Prompt reflects the profile working directory when one is set.
fn write_session_prompt(session: &mut Session) -> rt::Result<()> {
    let profile = session.profile;
    if profile.cwd_len == 0 {
        return emit_output(session, SHELL_PROMPT);
    }
    let mut buffer = rt::FixedLogBuffer::<64>::new();
    let cwd = core::str::from_utf8(&profile.cwd[..profile.cwd_len]).unwrap_or("");
    let _ = write!(&mut buffer, "{}{} ", cwd, SHELL_PROMPT);
    emit_output_line(session, buffer.as_bytes())
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
    if session.spare_endpoint != rt::INVALID_HANDLE {
        let _ = rt::handle_close(session.spare_endpoint);
    }
    *session = Session::empty();
}

/// Detach the attached client without disturbing shell state. Returns false
/// when the session was not attached.
pub(crate) fn detach_session(session: &mut Session) -> bool {
    session.mark_detached()
}

/// Mint a client handle for a reattach and restore the pane view. Returns
/// None when the session is busy (already attached) or handle duplication
/// failed; the caller replies Busy in that case.
pub(crate) fn attach_client(session: &mut Session) -> Option<rt::Handle> {
    if !session.can_attach() {
        return None;
    }
    match rt::handle_duplicate(session.spare_endpoint, ATTACH_RIGHTS) {
        Ok(handle) => {
            session.attached = true;
            replay_scrollback(session);
            redraw_input_line_fresh(session);
            Some(handle)
        }
        Err(_) => None,
    }
}

/// Replay retained output so an attaching pane rebuilds its grid contents.
fn replay_scrollback(session: &mut Session) {
    let mut snapshot = [0u8; SCROLLBACK_BYTES];
    let total = {
        let (first, second) = session.scrollback.slices();
        snapshot[..first.len()].copy_from_slice(first);
        snapshot[first.len()..first.len() + second.len()].copy_from_slice(second);
        first.len() + second.len()
    };
    let endpoint = session.endpoint;
    let _ = output_bytes(endpoint, &snapshot[..total]);
}

/// Bookmark the current input line for later re-edit. Returns whether a new
/// bookmark was stored.
pub(crate) fn bookmark_current_line(session: &mut Session) -> bool {
    if !session.attached || session.line_len == 0 {
        return false;
    }
    let mut line = [0u8; MAX_LINE_BYTES];
    let len = session.line_len;
    line[..len].copy_from_slice(&session.line[..len]);
    let stored = session.bookmarks.add(&line[..len]);
    if stored {
        session.history_view = None;
        let _ = emit_output(session, "\r\n[bm]\r\n");
        let _ = write_session_prompt(session);
    }
    stored
}

/// Cycle bookmarks into the editable input line (never auto-executed).
pub(crate) fn bookmark_cycle(session: &mut Session) -> rt::Result<()> {
    let mut entry = [0u8; MAX_LINE_BYTES];
    let Some(len) = session.bookmarks.cycle_next(&mut entry) else {
        return Ok(());
    };
    session.line[..len].copy_from_slice(&entry[..len]);
    session.line_len = len;
    session.line_cursor = len;
    session.history_view = None;
    redraw_input_line(session)
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

fn redraw_input_line(session: &mut Session) -> rt::Result<()> {
    emit_output(session, "\r\x1b[2K")?;
    write_session_prompt(session)?;
    if session.line_len > 0 {
        let mut bytes = [0u8; MAX_LINE_BYTES];
        let len = session.line_len;
        bytes[..len].copy_from_slice(&session.line[..len]);
        emit_output_line(session, &bytes[..len])?;
    }
    let tail = session.line_len.saturating_sub(session.line_cursor);
    if tail > 0 {
        write_cursor_left(session.endpoint, tail)?;
    }
    Ok(())
}

/// Rebuild the input line on a freshly attached pane without duplicating it
/// into the scrollback ring.
pub(crate) fn redraw_input_line_fresh(session: &mut Session) {
    let endpoint = session.endpoint;
    let profile = session.profile;
    let line_len = session.line_len;
    let cursor = session.line_cursor;
    let _ = terminal_output_write(endpoint, "\r\x1b[2K");
    if profile.cwd_len == 0 {
        let _ = terminal_output_write(endpoint, SHELL_PROMPT);
    } else {
        let mut buffer = rt::FixedLogBuffer::<64>::new();
        let cwd = core::str::from_utf8(&profile.cwd[..profile.cwd_len]).unwrap_or("");
        let _ = write!(&mut buffer, "{}{} ", cwd, SHELL_PROMPT);
        let _ = output_bytes(endpoint, buffer.as_bytes());
    }
    if line_len > 0 {
        let mut bytes = [0u8; MAX_LINE_BYTES];
        bytes[..line_len].copy_from_slice(&session.line[..line_len]);
        let _ = output_bytes(endpoint, &bytes[..line_len]);
    }
    let tail = line_len.saturating_sub(cursor);
    if tail > 0 {
        let _ = write_cursor_left(endpoint, tail);
    }
}

fn clear_line(session: &mut Session) {
    session.line_len = 0;
    session.line_cursor = 0;
    session.history_view = None;
    session.history_stash_len = 0;
    session.escape_state = EscapeState::None;
    session.bookmarks.reset_view();
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
