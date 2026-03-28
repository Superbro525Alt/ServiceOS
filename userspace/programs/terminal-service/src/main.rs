#![no_std]
#![no_main]

use core::fmt::Write;

use serviceos_shell_service::{
    SHELL_PROMPT, SHELL_READY_TEXT, ShellOutput, execute_command_with_source,
    write_output_linef,
};
use serviceos_userspace_runtime as rt;
use rt::{
    ConsoleTag, ControlTag, FixedLogBuffer, LifecycleEvent, LogDomain, LogEvent, LogSeverity,
    RawMessage, ServiceId, TerminalStatus, TerminalTag,
};

const MAX_SESSIONS: usize = 4;
const MAX_LINE_BYTES: usize = 128;
const MAX_HISTORY: usize = 16;
const MAX_INLINE_BYTES: usize = (rt::IPC_MAX_WORDS - 1) * 8;
const DEFAULT_COLS: u32 = 80;
const DEFAULT_ROWS: u32 = 25;

#[derive(Clone, Copy, Eq, PartialEq)]
enum EscapeState {
    None,
    Esc,
    Csi,
}

#[derive(Clone, Copy)]
struct Session {
    endpoint: rt::Handle,
    id: u32,
    columns: u32,
    rows: u32,
    width_pixels: u32,
    height_pixels: u32,
    line: [u8; MAX_LINE_BYTES],
    line_len: usize,
    line_cursor: usize,
    history: [[u8; MAX_LINE_BYTES]; MAX_HISTORY],
    history_lens: [usize; MAX_HISTORY],
    history_count: usize,
    history_head: usize,
    history_view: Option<usize>,
    history_stash: [u8; MAX_LINE_BYTES],
    history_stash_len: usize,
    escape_state: EscapeState,
    occupied: bool,
}

impl Session {
    const fn empty() -> Self {
        Self {
            endpoint: rt::INVALID_HANDLE,
            id: 0,
            columns: DEFAULT_COLS,
            rows: DEFAULT_ROWS,
            width_pixels: 0,
            height_pixels: 0,
            line: [0; MAX_LINE_BYTES],
            line_len: 0,
            line_cursor: 0,
            history: [[0; MAX_LINE_BYTES]; MAX_HISTORY],
            history_lens: [0; MAX_HISTORY],
            history_count: 0,
            history_head: 0,
            history_view: None,
            history_stash: [0; MAX_LINE_BYTES],
            history_stash_len: 0,
            escape_state: EscapeState::None,
            occupied: false,
        }
    }
}

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf901;
    }
    if startup.tag != ControlTag::Startup as u32 {
        return 0xf902;
    }

    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return 0xf903,
    };
    if rt::register_service(bootstrap, ServiceId::Terminal, public.second).is_err() {
        return 0xf904;
    }
    let _ = rt::handle_close(public.second);

    let mut sessions = [Session::empty(); MAX_SESSIONS];
    let mut next_session_id = 1u32;

    loop {
        match poll_lifecycle(bootstrap) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(_) => return 0xf905,
        }

        let mut request = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut request) {
            Ok(()) => {
                if handle_public_request(bootstrap, &mut sessions, &mut next_session_id, &request)
                    .is_err()
                {
                    return 0xf906;
                }
            }
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => return 0xf907,
        }

        for session in &mut sessions {
            if !session.occupied {
                continue;
            }
            loop {
                let mut message = RawMessage::empty(0);
                match rt::channel_receive_nonblocking(session.endpoint, &mut message) {
                    Ok(()) => {
                        if handle_session_message(bootstrap, session, &message).is_err() {
                            release_session(bootstrap, session);
                            break;
                        }
                    }
                    Err(rt::Error::QueueEmpty) => break,
                    Err(_) => {
                        release_session(bootstrap, session);
                        break;
                    }
                }
            }
        }

        if rt::yield_current().is_err() {
            return 0xf908;
        }
    }
}

fn handle_public_request(
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
            if let Some(session) = sessions.iter_mut().find(|session| !session.occupied) {
                let pair = rt::channel_create()?;
                session.endpoint = pair.first;
                session.id = *next_session_id;
                *next_session_id = next_session_id.saturating_add(1);
                session.columns = DEFAULT_COLS;
                session.rows = DEFAULT_ROWS;
                session.line_len = 0;
                session.line_cursor = 0;
                session.history_view = None;
                session.history_stash_len = 0;
                session.escape_state = EscapeState::None;
                session.occupied = true;

                reply.words[0] = TerminalStatus::Ok as u32 as u64;
                reply.words[1] = session.id as u64;
                reply.words[2] = session.columns as u64;
                reply.words[3] = session.rows as u64;
                reply.handle_count = 1;
                reply.handles[0] = pair.second;
                reply.handle_rights[0] =
                    rt::rights::SEND | rt::rights::RECEIVE | rt::rights::DUPLICATE | rt::rights::TRANSFER;
                let _ = rt::channel_send(reply_handle, &reply);
                let _ = rt::handle_close(pair.second);
                let _ = emit_terminal_log(
                    bootstrap,
                    LogSeverity::Info,
                    LogEvent::TerminalSessionOpened,
                    session.id as u64,
                    0,
                );
                let _ = terminal_output_write(pair.first, SHELL_READY_TEXT);
                let _ = terminal_output_write(pair.first, "\r\n");
                let _ = terminal_output_write(pair.first, SHELL_PROMPT);
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

fn handle_session_message(
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
            unpack_bytes(&message.words[1..message.word_count as usize], len, &mut buffer)?;
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

fn handle_input_byte(bootstrap: rt::Handle, session: &mut Session, byte: u8) -> rt::Result<()> {
    if byte == 0x03 {
        terminal_output_write(session.endpoint, "^C\r\n")?;
        clear_line(session);
        terminal_output_write(session.endpoint, SHELL_PROMPT)?;
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
            terminal_output_write(session.endpoint, "\r\n")?;
            let line_len = session.line_len;
            if line_len > 0 {
                append_history(session);
                let line = core::str::from_utf8(&session.line[..line_len])
                    .map_err(|_| rt::Error::InvalidArgument)?;
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    let output = ShellOutput::new(session.endpoint, terminal_output_write);
                    if let Err(error) = execute_command_with_source(
                        bootstrap,
                        ServiceId::Terminal,
                        output,
                        trimmed,
                    ) {
                        let _ = write_output_linef(
                            output,
                            format_args!("command failed: {}", serviceos_shell_service::util::error_name(error)),
                        );
                    }
                }
            }
            clear_line(session);
            terminal_output_write(session.endpoint, SHELL_PROMPT)?;
        }
        0x08 | 0x7f => backspace(session)?,
        0x20..=0x7e => insert_byte(session, byte)?,
        _ => {}
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
    terminal_output_write(session.endpoint, SHELL_PROMPT)?;
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

fn release_session(bootstrap: rt::Handle, session: &mut Session) {
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

fn write_cursor_left(endpoint: rt::Handle, count: usize) -> rt::Result<()> {
    if count == 0 {
        return Ok(());
    }
    let mut buffer = FixedLogBuffer::<16>::new();
    let _ = write!(&mut buffer, "\x1b[{}D", count);
    let text = core::str::from_utf8(buffer.as_bytes()).map_err(|_| rt::Error::InvalidArgument)?;
    terminal_output_write(endpoint, text)
}

fn terminal_output_write(endpoint: rt::Handle, text: &str) -> rt::Result<()> {
    let bytes = text.as_bytes();
    let mut offset = 0usize;
    while offset < bytes.len() {
        let end = (offset + MAX_INLINE_BYTES).min(bytes.len());
        let chunk = &bytes[offset..end];
        let mut message = RawMessage::empty(ConsoleTag::SessionWriteText as u32);
        message.word_count = 1 + pack_bytes(chunk, &mut message.words[1..])?;
        message.words[0] = chunk.len() as u64;
        rt::channel_send(endpoint, &message)?;
        offset = end;
    }
    Ok(())
}

fn pack_bytes(source: &[u8], words: &mut [u64]) -> rt::Result<u32> {
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

fn poll_lifecycle(bootstrap: rt::Handle) -> rt::Result<bool> {
    let mut message = RawMessage::empty(0);
    match rt::channel_receive_nonblocking(bootstrap, &mut message) {
        Ok(()) if message.tag == ControlTag::Lifecycle as u32 && message.word_count > 0 => Ok(
            matches!(
                lifecycle_event_from_word(message.words[0]),
                LifecycleEvent::Restarting | LifecycleEvent::Stopped
            ),
        ),
        Ok(()) => Ok(false),
        Err(rt::Error::QueueEmpty) => Ok(false),
        Err(error) => Err(error),
    }
}

fn lifecycle_event_from_word(value: u64) -> LifecycleEvent {
    match value as u32 {
        x if x == LifecycleEvent::Starting as u32 => LifecycleEvent::Starting,
        x if x == LifecycleEvent::Ready as u32 => LifecycleEvent::Ready,
        x if x == LifecycleEvent::Failed as u32 => LifecycleEvent::Failed,
        x if x == LifecycleEvent::Stopped as u32 => LifecycleEvent::Stopped,
        _ => LifecycleEvent::Restarting,
    }
}

fn emit_terminal_log(
    bootstrap: rt::Handle,
    severity: LogSeverity,
    event: LogEvent,
    arg0: u64,
    arg1: u64,
) -> rt::Result<()> {
    let log_handle = rt::lookup_service(bootstrap, ServiceId::Log)?;
    let result = rt::send_log_record(
        log_handle,
        ServiceId::Terminal,
        severity,
        LogDomain::Shell,
        event,
        arg0,
        arg1,
    );
    let _ = rt::handle_close(log_handle);
    result
}
