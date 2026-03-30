#![no_std]
#![no_main]

use core::fmt::Write;

use serviceos_userspace_runtime as rt;
use rt::{
    rights, ConsoleTag, ControlTag, LifecycleEvent, LogDomain, LogEvent, LogSeverity, RawMessage,
    ServiceId,
};

const MAX_SESSIONS: usize = 2;
const MAX_LINE_BYTES: usize = 128;
const MAX_DISPLAY_BYTES: usize = 192;
const MAX_HISTORY: usize = 16;

#[derive(Clone, Copy, Eq, PartialEq)]
enum EscapeState {
    None,
    Esc,
    Csi,
}

#[derive(Clone, Copy)]
struct Session {
    endpoint: rt::Handle,
    pending_reply: rt::Handle,
    line: [u8; MAX_LINE_BYTES],
    line_len: usize,
    line_cursor: usize,
    display: [u8; MAX_DISPLAY_BYTES],
    display_len: usize,
    prompt_len: usize,
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
            pending_reply: rt::INVALID_HANDLE,
            line: [0; MAX_LINE_BYTES],
            line_len: 0,
            line_cursor: 0,
            display: [0; MAX_DISPLAY_BYTES],
            display_len: 0,
            prompt_len: 0,
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
        return 0xf301;
    }

    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return 0xf302,
    };
    if rt::register_service(bootstrap, ServiceId::Console, public.second).is_err() {
        return 0xf303;
    }
    let _ = rt::handle_close(public.second);

    let mut sessions = [Session::empty(); MAX_SESSIONS];
    loop {
        match poll_lifecycle(bootstrap) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(_) => return 0xf304,
        }

        let mut message = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut message) {
            Ok(()) => {
                if handle_public_message(&mut sessions, &message).is_err() {
                    return 0xf305;
                }
            }
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => return 0xf306,
        }

        for session in &mut sessions {
            if !session.occupied {
                continue;
            }
            let mut session_message = RawMessage::empty(0);
            match rt::channel_receive_nonblocking(session.endpoint, &mut session_message) {
                Ok(()) => {
                    if handle_session_message(session, &session_message).is_err() {
                        return 0xf307;
                    }
                }
                Err(rt::Error::QueueEmpty) => {}
                Err(_) => release_session(session),
            }
        }

        loop {
            match rt::debug_console_read_byte() {
                Ok(byte) => {
                    if handle_input_byte(&mut sessions, byte).is_err() {
                        return 0xf309;
                    }
                }
                Err(rt::Error::QueueEmpty) => break,
                Err(_) => return 0xf30a,
            }
        }

        if rt::yield_current().is_err() {
            return 0xf30b;
        }
    }
}

fn handle_public_message(
    sessions: &mut [Session; MAX_SESSIONS],
    message: &RawMessage,
) -> rt::Result<()> {
    match message.tag {
        x if x == ConsoleTag::WriteRecord as u32 => {
            if message.word_count < 7 {
                return Ok(());
            }

            let source = service_id_from_word(message.words[0]);
            let severity = severity_from_word(message.words[1]);
            let domain = domain_from_word(message.words[2]);
            let event = event_from_word(message.words[3]);
            let _ = match event {
                LogEvent::ServiceStarted | LogEvent::ServiceReady | LogEvent::ServiceRestarting => {
                    write_structured_line(
                        sessions,
                        "console",
                        format_args!(
                            "seq={} level={} source={} domain={} event={} service={} detail={}",
                            message.words[6],
                            severity_name(severity),
                            service_name(source),
                            domain_name(domain),
                            event_name(event),
                            service_name(service_id_from_word(message.words[4])),
                            message.words[5],
                        ),
                    )
                }
                LogEvent::ServiceFailed => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} service={} exit={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        service_name(service_id_from_word(message.words[4])),
                        message.words[5],
                    ),
                ),
                LogEvent::LookupGranted => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} requester={} target={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        service_name(service_id_from_word(message.words[4])),
                        service_name(service_id_from_word(message.words[5])),
                    ),
                ),
                LogEvent::ConfigLoaded => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} minimum-severity={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        message.words[4],
                    ),
                ),
                LogEvent::StatusStarted => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} heartbeat-ticks={} console-period={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        message.words[4],
                        message.words[5],
                    ),
                ),
                LogEvent::StatusHeartbeat | LogEvent::ConsoleWrite => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} count={} tick={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        message.words[4],
                        message.words[5],
                    ),
                ),
                LogEvent::ConfigRead => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} key={} value={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        message.words[4],
                        message.words[5],
                    ),
                ),
                LogEvent::NetworkInterfaceReady => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} iface={} mac={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        message.words[4],
                        format_mac(unpack_mac(message.words[5])),
                    ),
                ),
                LogEvent::NetworkAddressConfigured => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} addr={} gateway={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        format_ipv4(message.words[4] as u32),
                        format_ipv4(message.words[5] as u32),
                    ),
                ),
                LogEvent::NetworkResolveCompleted => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} addr={} count={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        format_ipv4(message.words[4] as u32),
                        message.words[5],
                    ),
                ),
                LogEvent::NetworkProbeCompleted => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} addr={} elapsed-ms={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        format_ipv4(message.words[4] as u32),
                        message.words[5],
                    ),
                ),
                LogEvent::DisplayOutputReady => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} mode={}x{}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        message.words[4],
                        message.words[5],
                    ),
                ),
                LogEvent::SurfaceCreated => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} surface={} session={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        message.words[4],
                        message.words[5],
                    ),
                ),
                LogEvent::CompositorPresented => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} surfaces={} presents={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        message.words[4],
                        message.words[5],
                    ),
                ),
                LogEvent::SessionReady => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} session={} surfaces={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        message.words[4],
                        message.words[5],
                    ),
                ),
                LogEvent::SessionFocusChanged => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} session={} surface={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        message.words[4],
                        message.words[5],
                    ),
                ),
                LogEvent::DesktopReady => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} session={} width={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        message.words[4],
                        message.words[5],
                    ),
                ),
                LogEvent::DesktopAppLaunched
                | LogEvent::DesktopAppExited
                | LogEvent::DesktopFocusChanged
                | LogEvent::AppRendered
                | LogEvent::InputKeyDelivered => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} app={} detail={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        message.words[4],
                        message.words[5],
                    ),
                ),
                _ => write_structured_line(
                    sessions,
                    "console",
                    format_args!(
                        "seq={} level={} source={} domain={} event={} detail0={} detail1={}",
                        message.words[6],
                        severity_name(severity),
                        service_name(source),
                        domain_name(domain),
                        event_name(event),
                        message.words[4],
                        message.words[5],
                    ),
                ),
            };
        }
        x if x == ConsoleTag::SessionOpenRequest as u32 => {
            if message.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = message.handles[0];
            let mut reply = RawMessage::empty(ConsoleTag::SessionOpenReply as u32);

            if let Some(session) = sessions.iter_mut().find(|session| !session.occupied) {
                let pair = rt::channel_create()?;
                session.endpoint = pair.first;
                reset_input_state(session);
                session.occupied = true;

                reply.handle_count = 1;
                reply.handles[0] = pair.second;
                reply.handle_rights[0] =
                    rights::SEND | rights::RECEIVE | rights::DUPLICATE | rights::TRANSFER;
                let _ = rt::channel_send(reply_handle, &reply);
                let _ = rt::handle_close(pair.second);
            } else {
                let _ = rt::channel_send(reply_handle, &reply);
            }

            let _ = rt::handle_close(reply_handle);
        }
        _ => {}
    }

    Ok(())
}

fn handle_session_message(session: &mut Session, message: &RawMessage) -> rt::Result<()> {
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

fn handle_input_byte(sessions: &mut [Session; MAX_SESSIONS], byte: u8) -> rt::Result<()> {
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
        0x1b => {
            session.escape_state = EscapeState::Esc;
        }
        b'\r' | b'\n' => {
            let _ = rt::debug_console_write(b"\r\n");
            let reply_len = session.line_len;
            if reply_len > 0 {
                append_history(session);
            }
            reply_with_current_line(session, reply_len)?;
        }
        0x08 | 0x7f => {
            backspace(session)?;
        }
        0x20..=0x7e => {
            insert_byte(session, byte)?;
        }
        _ => {}
    }

    Ok(())
}

fn begin_input_session(session: &mut Session) {
    session.line_len = 0;
    session.line_cursor = 0;
    session.prompt_len = session.display_len;
    session.history_view = None;
    session.history_stash_len = 0;
    session.escape_state = EscapeState::None;
}

fn reset_input_state(session: &mut Session) {
    if session.pending_reply != rt::INVALID_HANDLE {
        let _ = rt::handle_close(session.pending_reply);
    }
    session.pending_reply = rt::INVALID_HANDLE;
    session.line_len = 0;
    session.line_cursor = 0;
    session.prompt_len = 0;
    session.display_len = 0;
    session.history_view = None;
    session.history_stash_len = 0;
    session.escape_state = EscapeState::None;
}

fn reply_with_current_line(session: &mut Session, reply_len: usize) -> rt::Result<()> {
    let mut reply = RawMessage::empty(ConsoleTag::SessionReadLineReply as u32);
    reply.word_count = 1;
    reply.words[0] = reply_len as u64;
    reply.word_count += pack_bytes(&session.line[..reply_len], &mut reply.words[1..])?;
    let _ = rt::channel_send(session.pending_reply, &reply);
    let _ = rt::handle_close(session.pending_reply);
    session.pending_reply = rt::INVALID_HANDLE;
    session.line_len = 0;
    session.line_cursor = 0;
    session.prompt_len = 0;
    session.display_len = 0;
    session.history_view = None;
    session.history_stash_len = 0;
    session.escape_state = EscapeState::None;
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
            session.history_stash[..session.line_len].copy_from_slice(&session.line[..session.line_len]);
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
        if latest_len == session.line_len && session.history[latest_slot][..latest_len] == session.line[..session.line_len] {
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
    let keep = session.prompt_len.min(session.display_len).min(session.display.len());
    let prompt = session.display;
    let mut next = [0u8; MAX_DISPLAY_BYTES];
    next[..keep].copy_from_slice(&prompt[..keep]);
    let line_copy = session.line_len.min(next.len().saturating_sub(keep));
    next[keep..keep + line_copy].copy_from_slice(&session.line[..line_copy]);
    session.display = next;
    session.display_len = keep + line_copy;
}

fn render_session_line(session: &Session) -> rt::Result<()> {
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

fn write_session_bytes(session: &mut Session, bytes: &[u8]) -> rt::Result<()> {
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

fn write_structured_line(
    sessions: &[Session; MAX_SESSIONS],
    domain: &str,
    args: core::fmt::Arguments<'_>,
) -> rt::Result<()> {
    if active_session(sessions).is_some() {
        let _ = rt::debug_console_write(b"\r\n");
    }
    rt::write_logf(domain, args)?;
    if let Some(session) = active_session(sessions) {
        let _ = render_session_line(session);
    }
    Ok(())
}

fn active_session(sessions: &[Session; MAX_SESSIONS]) -> Option<&Session> {
    sessions
        .iter()
        .find(|session| {
            session.occupied
                && session.pending_reply != rt::INVALID_HANDLE
                && session.display_len > 0
        })
}

fn push_display_byte(session: &mut Session, byte: u8) {
    if matches!(byte, 0x20..=0x7e) && session.display_len < session.display.len() {
        session.display[session.display_len] = byte;
        session.display_len += 1;
    }
}

fn pop_display_byte(session: &mut Session) {
    if session.display_len > 0 {
        session.display_len -= 1;
    }
}

fn release_session(session: &mut Session) {
    let endpoint = session.endpoint;
    reset_input_state(session);
    if endpoint != rt::INVALID_HANDLE {
        let _ = rt::handle_close(endpoint);
    }
    *session = Session::empty();
}

fn poll_lifecycle(bootstrap: rt::Handle) -> rt::Result<bool> {
    let mut message = RawMessage::empty(0);
    match rt::channel_receive_nonblocking(bootstrap, &mut message) {
        Ok(()) if message.tag == ControlTag::Lifecycle as u32 && message.word_count > 0 => {
            Ok(matches!(
                lifecycle_event_from_word(message.words[0]),
                LifecycleEvent::Restarting | LifecycleEvent::Stopped
            ))
        }
        Ok(()) => Ok(false),
        Err(rt::Error::QueueEmpty) => Ok(false),
        Err(error) => Err(error),
    }
}

fn service_id_from_word(value: u64) -> ServiceId {
    match value as u32 {
        x if x == ServiceId::Storage as u32 => ServiceId::Storage,
        x if x == ServiceId::Console as u32 => ServiceId::Console,
        x if x == ServiceId::Config as u32 => ServiceId::Config,
        x if x == ServiceId::Log as u32 => ServiceId::Log,
        x if x == ServiceId::Status as u32 => ServiceId::Status,
        x if x == ServiceId::Shell as u32 => ServiceId::Shell,
        x if x == ServiceId::Package as u32 => ServiceId::Package,
        x if x == ServiceId::Announce as u32 => ServiceId::Announce,
        x if x == ServiceId::Network as u32 => ServiceId::Network,
        x if x == ServiceId::Graphics as u32 => ServiceId::Graphics,
        x if x == ServiceId::Session as u32 => ServiceId::Session,
        x if x == ServiceId::DesktopShell as u32 => ServiceId::DesktopShell,
        x if x == ServiceId::Terminal as u32 => ServiceId::Terminal,
        x if x == ServiceId::Audio as u32 => ServiceId::Audio,
        x if x == ServiceId::Runtime as u32 => ServiceId::Runtime,
        x if x == ServiceId::Developer as u32 => ServiceId::Developer,
        x if x == ServiceId::Clipboard as u32 => ServiceId::Clipboard,
        _ => ServiceId::RootManager,
    }
}

fn severity_from_word(value: u64) -> LogSeverity {
    match value as u32 {
        x if x == LogSeverity::Trace as u32 => LogSeverity::Trace,
        x if x == LogSeverity::Debug as u32 => LogSeverity::Debug,
        x if x == LogSeverity::Warn as u32 => LogSeverity::Warn,
        x if x == LogSeverity::Error as u32 => LogSeverity::Error,
        _ => LogSeverity::Info,
    }
}

fn domain_from_word(value: u64) -> LogDomain {
    match value as u32 {
        x if x == LogDomain::Bootstrap as u32 => LogDomain::Bootstrap,
        x if x == LogDomain::ServiceManager as u32 => LogDomain::ServiceManager,
        x if x == LogDomain::Storage as u32 => LogDomain::Storage,
        x if x == LogDomain::Log as u32 => LogDomain::Log,
        x if x == LogDomain::Config as u32 => LogDomain::Config,
        x if x == LogDomain::Console as u32 => LogDomain::Console,
        x if x == LogDomain::Status as u32 => LogDomain::Status,
        x if x == LogDomain::Ipc as u32 => LogDomain::Ipc,
        x if x == LogDomain::Shell as u32 => LogDomain::Shell,
        x if x == LogDomain::Package as u32 => LogDomain::Package,
        x if x == LogDomain::Network as u32 => LogDomain::Network,
        x if x == LogDomain::Graphics as u32 => LogDomain::Graphics,
        x if x == LogDomain::Session as u32 => LogDomain::Session,
        x if x == LogDomain::Desktop as u32 => LogDomain::Desktop,
        x if x == LogDomain::App as u32 => LogDomain::App,
        x if x == LogDomain::Audio as u32 => LogDomain::Audio,
        x if x == LogDomain::Runtime as u32 => LogDomain::Runtime,
        x if x == LogDomain::Developer as u32 => LogDomain::Developer,
        _ => LogDomain::Service,
    }
}

fn event_from_word(value: u64) -> LogEvent {
    match value as u32 {
        x if x == LogEvent::ServiceStarted as u32 => LogEvent::ServiceStarted,
        x if x == LogEvent::ServiceReady as u32 => LogEvent::ServiceReady,
        x if x == LogEvent::ServiceFailed as u32 => LogEvent::ServiceFailed,
        x if x == LogEvent::ServiceRestarting as u32 => LogEvent::ServiceRestarting,
        x if x == LogEvent::ConfigLoaded as u32 => LogEvent::ConfigLoaded,
        x if x == LogEvent::ConfigRead as u32 => LogEvent::ConfigRead,
        x if x == LogEvent::ConsoleWrite as u32 => LogEvent::ConsoleWrite,
        x if x == LogEvent::StatusStarted as u32 => LogEvent::StatusStarted,
        x if x == LogEvent::StatusHeartbeat as u32 => LogEvent::StatusHeartbeat,
        x if x == LogEvent::StorageMounted as u32 => LogEvent::StorageMounted,
        x if x == LogEvent::ManifestLoaded as u32 => LogEvent::ManifestLoaded,
        x if x == LogEvent::ResourceOpened as u32 => LogEvent::ResourceOpened,
        x if x == LogEvent::SessionOpened as u32 => LogEvent::SessionOpened,
        x if x == LogEvent::ShellCommand as u32 => LogEvent::ShellCommand,
        x if x == LogEvent::ToolLaunched as u32 => LogEvent::ToolLaunched,
        x if x == LogEvent::PackageCatalogLoaded as u32 => LogEvent::PackageCatalogLoaded,
        x if x == LogEvent::PackageInstalled as u32 => LogEvent::PackageInstalled,
        x if x == LogEvent::PackageUpdated as u32 => LogEvent::PackageUpdated,
        x if x == LogEvent::PackageRemoved as u32 => LogEvent::PackageRemoved,
        x if x == LogEvent::PackageRolledBack as u32 => LogEvent::PackageRolledBack,
        x if x == LogEvent::PackageActivationFailed as u32 => LogEvent::PackageActivationFailed,
        x if x == LogEvent::NetworkInterfaceReady as u32 => LogEvent::NetworkInterfaceReady,
        x if x == LogEvent::NetworkAddressConfigured as u32 => LogEvent::NetworkAddressConfigured,
        x if x == LogEvent::NetworkResolveCompleted as u32 => LogEvent::NetworkResolveCompleted,
        x if x == LogEvent::NetworkProbeCompleted as u32 => LogEvent::NetworkProbeCompleted,
        x if x == LogEvent::NetworkLinkChanged as u32 => LogEvent::NetworkLinkChanged,
        x if x == LogEvent::DisplayOutputReady as u32 => LogEvent::DisplayOutputReady,
        x if x == LogEvent::SurfaceCreated as u32 => LogEvent::SurfaceCreated,
        x if x == LogEvent::SurfaceUpdated as u32 => LogEvent::SurfaceUpdated,
        x if x == LogEvent::CompositorPresented as u32 => LogEvent::CompositorPresented,
        x if x == LogEvent::SessionReady as u32 => LogEvent::SessionReady,
        x if x == LogEvent::SessionFocusChanged as u32 => LogEvent::SessionFocusChanged,
        x if x == LogEvent::DesktopReady as u32 => LogEvent::DesktopReady,
        x if x == LogEvent::DesktopAppLaunched as u32 => LogEvent::DesktopAppLaunched,
        x if x == LogEvent::DesktopAppExited as u32 => LogEvent::DesktopAppExited,
        x if x == LogEvent::DesktopFocusChanged as u32 => LogEvent::DesktopFocusChanged,
        x if x == LogEvent::AppRendered as u32 => LogEvent::AppRendered,
        x if x == LogEvent::InputSourceReady as u32 => LogEvent::InputSourceReady,
        x if x == LogEvent::InputKeyDelivered as u32 => LogEvent::InputKeyDelivered,
        x if x == LogEvent::TerminalSessionOpened as u32 => LogEvent::TerminalSessionOpened,
        x if x == LogEvent::TerminalSessionClosed as u32 => LogEvent::TerminalSessionClosed,
        x if x == LogEvent::AudioEndpointReady as u32 => LogEvent::AudioEndpointReady,
        x if x == LogEvent::AudioStreamOpened as u32 => LogEvent::AudioStreamOpened,
        x if x == LogEvent::AudioStreamStarted as u32 => LogEvent::AudioStreamStarted,
        x if x == LogEvent::AudioStreamStopped as u32 => LogEvent::AudioStreamStopped,
        x if x == LogEvent::AudioStreamClosed as u32 => LogEvent::AudioStreamClosed,
        x if x == LogEvent::RuntimeEnvironmentCreated as u32 => LogEvent::RuntimeEnvironmentCreated,
        x if x == LogEvent::RuntimeEnvironmentDestroyed as u32 => {
            LogEvent::RuntimeEnvironmentDestroyed
        }
        x if x == LogEvent::RuntimeLaunchStarted as u32 => LogEvent::RuntimeLaunchStarted,
        x if x == LogEvent::RuntimeLaunchExited as u32 => LogEvent::RuntimeLaunchExited,
        x if x == LogEvent::RuntimeMappedRead as u32 => LogEvent::RuntimeMappedRead,
        x if x == LogEvent::DeveloperCatalogLoaded as u32 => LogEvent::DeveloperCatalogLoaded,
        x if x == LogEvent::DeveloperBuildStarted as u32 => LogEvent::DeveloperBuildStarted,
        x if x == LogEvent::DeveloperBuildFinished as u32 => LogEvent::DeveloperBuildFinished,
        x if x == LogEvent::DeveloperBuildFailed as u32 => LogEvent::DeveloperBuildFailed,
        x if x == LogEvent::DeveloperArtifactOpened as u32 => LogEvent::DeveloperArtifactOpened,
        _ => LogEvent::LookupGranted,
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

fn service_name(service_id: ServiceId) -> &'static str {
    match service_id {
        ServiceId::RootManager => "root-manager",
        ServiceId::Storage => "storage-service",
        ServiceId::Console => "console-service",
        ServiceId::Config => "config-service",
        ServiceId::Log => "log-service",
        ServiceId::Status => "status-service",
        ServiceId::Shell => "shell-service",
        ServiceId::Package => "package-service",
        ServiceId::Announce => "announce-service",
        ServiceId::Network => "network-service",
        ServiceId::Graphics => "graphics-service",
        ServiceId::Session => "session-service",
        ServiceId::DesktopShell => "desktop-shell-service",
        ServiceId::Terminal => "terminal-service",
        ServiceId::Audio => "audio-service",
        ServiceId::Runtime => "runtime-service",
        ServiceId::Developer => "developer-service",
        ServiceId::Clipboard => "clipboard-service",
    }
}

fn severity_name(severity: LogSeverity) -> &'static str {
    match severity {
        LogSeverity::Trace => "trace",
        LogSeverity::Debug => "debug",
        LogSeverity::Info => "info",
        LogSeverity::Warn => "warn",
        LogSeverity::Error => "error",
    }
}

fn domain_name(domain: LogDomain) -> &'static str {
    match domain {
        LogDomain::Bootstrap => "bootstrap",
        LogDomain::ServiceManager => "service-manager",
        LogDomain::Service => "service",
        LogDomain::Storage => "storage",
        LogDomain::Log => "log",
        LogDomain::Config => "config",
        LogDomain::Console => "console",
        LogDomain::Status => "status",
        LogDomain::Ipc => "ipc",
        LogDomain::Shell => "shell",
        LogDomain::Package => "package",
        LogDomain::Network => "network",
        LogDomain::Graphics => "graphics",
        LogDomain::Session => "session",
        LogDomain::Desktop => "desktop",
        LogDomain::App => "app",
        LogDomain::Audio => "audio",
        LogDomain::Runtime => "runtime",
        LogDomain::Developer => "developer",
    }
}

fn event_name(event: LogEvent) -> &'static str {
    match event {
        LogEvent::ServiceStarted => "service-started",
        LogEvent::ServiceReady => "service-ready",
        LogEvent::ServiceFailed => "service-failed",
        LogEvent::ServiceRestarting => "service-restarting",
        LogEvent::ConfigLoaded => "config-loaded",
        LogEvent::ConfigRead => "config-read",
        LogEvent::ConsoleWrite => "console-write",
        LogEvent::StatusStarted => "status-started",
        LogEvent::StatusHeartbeat => "status-heartbeat",
        LogEvent::LookupGranted => "lookup-granted",
        LogEvent::StorageMounted => "storage-mounted",
        LogEvent::ManifestLoaded => "manifest-loaded",
        LogEvent::ResourceOpened => "resource-opened",
        LogEvent::SessionOpened => "session-opened",
        LogEvent::ShellCommand => "shell-command",
        LogEvent::ToolLaunched => "tool-launched",
        LogEvent::PackageCatalogLoaded => "package-catalog-loaded",
        LogEvent::PackageInstalled => "package-installed",
        LogEvent::PackageUpdated => "package-updated",
        LogEvent::PackageRemoved => "package-removed",
        LogEvent::PackageRolledBack => "package-rolled-back",
        LogEvent::PackageActivationFailed => "package-activation-failed",
        LogEvent::NetworkInterfaceReady => "network-interface-ready",
        LogEvent::NetworkAddressConfigured => "network-address-configured",
        LogEvent::NetworkResolveCompleted => "network-resolve-completed",
        LogEvent::NetworkProbeCompleted => "network-probe-completed",
        LogEvent::NetworkLinkChanged => "network-link-changed",
        LogEvent::NetworkLeaseChanged => "network-lease-changed",
        LogEvent::NetworkSocketOpened => "network-socket-opened",
        LogEvent::NetworkSocketClosed => "network-socket-closed",
        LogEvent::DisplayOutputReady => "display-output-ready",
        LogEvent::SurfaceCreated => "surface-created",
        LogEvent::SurfaceUpdated => "surface-updated",
        LogEvent::CompositorPresented => "compositor-presented",
        LogEvent::SessionReady => "session-ready",
        LogEvent::SessionFocusChanged => "session-focus-changed",
        LogEvent::DesktopReady => "desktop-ready",
        LogEvent::DesktopAppLaunched => "desktop-app-launched",
        LogEvent::DesktopAppExited => "desktop-app-exited",
        LogEvent::DesktopFocusChanged => "desktop-focus-changed",
        LogEvent::AppRendered => "app-rendered",
        LogEvent::InputSourceReady => "input-source-ready",
        LogEvent::InputKeyDelivered => "input-key-delivered",
        LogEvent::TerminalSessionOpened => "terminal-session-opened",
        LogEvent::TerminalSessionClosed => "terminal-session-closed",
        LogEvent::AudioEndpointReady => "audio-endpoint-ready",
        LogEvent::AudioStreamOpened => "audio-stream-opened",
        LogEvent::AudioStreamStarted => "audio-stream-started",
        LogEvent::AudioStreamStopped => "audio-stream-stopped",
        LogEvent::AudioStreamClosed => "audio-stream-closed",
        LogEvent::RuntimeEnvironmentCreated => "runtime-environment-created",
        LogEvent::RuntimeEnvironmentDestroyed => "runtime-environment-destroyed",
        LogEvent::RuntimeLaunchStarted => "runtime-launch-started",
        LogEvent::RuntimeLaunchExited => "runtime-launch-exited",
        LogEvent::RuntimeMappedRead => "runtime-mapped-read",
        LogEvent::DeveloperCatalogLoaded => "developer-catalog-loaded",
        LogEvent::DeveloperBuildStarted => "developer-build-started",
        LogEvent::DeveloperBuildFinished => "developer-build-finished",
        LogEvent::DeveloperBuildFailed => "developer-build-failed",
        LogEvent::DeveloperArtifactOpened => "developer-artifact-opened",
    }
}

fn format_ipv4(value: u32) -> FixedValueText {
    FixedValueText::ipv4(value)
}

fn format_mac(value: [u8; 6]) -> FixedValueText {
    FixedValueText::mac(value)
}

fn unpack_mac(value: u64) -> [u8; 6] {
    [
        (value & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        ((value >> 16) & 0xff) as u8,
        ((value >> 24) & 0xff) as u8,
        ((value >> 32) & 0xff) as u8,
        ((value >> 40) & 0xff) as u8,
    ]
}

struct FixedValueText {
    bytes: [u8; 32],
    len: usize,
}

impl FixedValueText {
    fn ipv4(value: u32) -> Self {
        let mut text = Self {
            bytes: [0; 32],
            len: 0,
        };
        let _ = write!(
            &mut text,
            "{}.{}.{}.{}",
            (value >> 24) & 0xff,
            (value >> 16) & 0xff,
            (value >> 8) & 0xff,
            value & 0xff,
        );
        text
    }

    fn mac(value: [u8; 6]) -> Self {
        let mut text = Self {
            bytes: [0; 32],
            len: 0,
        };
        let _ = write!(
            &mut text,
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            value[0], value[1], value[2], value[3], value[4], value[5],
        );
        text
    }
}

impl core::fmt::Display for FixedValueText {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let text = core::str::from_utf8(&self.bytes[..self.len]).map_err(|_| core::fmt::Error)?;
        f.write_str(text)
    }
}

impl Write for FixedValueText {
    fn write_str(&mut self, value: &str) -> core::fmt::Result {
        let bytes = value.as_bytes();
        let remaining = self.bytes.len().saturating_sub(self.len);
        let copy_len = remaining.min(bytes.len());
        self.bytes[self.len..self.len + copy_len].copy_from_slice(&bytes[..copy_len]);
        self.len += copy_len;
        Ok(())
    }
}
