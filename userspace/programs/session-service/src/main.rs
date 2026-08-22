#![no_std]
#![no_main]

use serviceos_userspace_runtime as rt;
use rt::{
    ControlTag, DesktopInputAction, InputButton, InputEventKind, LifecycleEvent, LogDomain,
    LogEvent, LogSeverity, RawMessage, ServiceId, SessionInputSource, SessionStatus, SessionTag,
};

const SESSION_ID: u32 = 1;
const MOD_SHIFT: u32 = 1 << 0;
const MOD_ALT: u32 = 1 << 1;
const MOD_CTRL: u32 = 1 << 2;
const MAX_SESSION_REQUESTS_PER_TURN: usize = 16;
const MAX_INPUT_EVENTS_PER_TURN: usize = 32;

const KEY_LEFT_SHIFT: u32 = 42;
const KEY_RIGHT_SHIFT: u32 = 54;
const KEY_LEFT_ALT: u32 = 56;
const KEY_RIGHT_ALT: u32 = 100;
const KEY_LEFT_CTRL: u32 = 29;
const KEY_RIGHT_CTRL: u32 = 97;

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xfd01;
    }
    if startup.tag != ControlTag::Startup as u32 || startup.handle_count < 2 {
        return 0xfd02;
    }

    let input_handle = startup.handles[0];
    let log_handle = startup.handles[1];

    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return 0xfd03,
    };
    if rt::register_service(bootstrap, ServiceId::Session, public.second).is_err() {
        return 0xfd04;
    }
    let _ = rt::handle_close(public.second);

    let output_size = match lookup_output_size(bootstrap) {
        Ok(size) => size,
        Err(_) => return 0xfd09,
    };
    let input_info = match rt::input_source_info(input_handle) {
        Ok(info) => info,
        Err(_) => return 0xfd0a,
    };

    let mut state = SessionState {
        focused_surface: 0,
        surface_count_hint: 0,
        input_source: if input_info.capabilities == 0 {
            SessionInputSource::ServiceControl
        } else {
            SessionInputSource::Hardware
        },
        input_handle,
        desktop_handle: rt::INVALID_HANDLE,
        output_width: output_size.0,
        output_height: output_size.1,
        pointer_x: (output_size.0 / 2) as i32,
        pointer_y: (output_size.1 / 2) as i32,
        modifiers: 0,
    };
    let _ = emit_log(
        log_handle,
        LogSeverity::Info,
        LogEvent::SessionReady,
        SESSION_ID as u64,
        0,
    );
    let _ = emit_log(
        log_handle,
        LogSeverity::Info,
        LogEvent::InputSourceReady,
        input_info.capabilities as u64,
        input_info.device_count as u64,
    );

    loop {
        let mut did_work = false;
        match poll_lifecycle(bootstrap) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(_) => return 0xfd05,
        }

        let mut request_budget = MAX_SESSION_REQUESTS_PER_TURN;
        loop {
            let mut request = RawMessage::empty(0);
            match rt::channel_receive_nonblocking(public.first, &mut request) {
                Ok(()) => {
                    did_work = true;
                    if handle_request(&request, log_handle, &mut state).is_err() {
                        return 0xfd06;
                    }
                    request_budget = request_budget.saturating_sub(1);
                    if request_budget == 0 {
                        break;
                    }
                }
                Err(rt::Error::QueueEmpty) => break,
                Err(_) => return 0xfd07,
            }
        }

        let allow_input_wait = !did_work && state.input_source == SessionInputSource::Hardware;
        match poll_input(bootstrap, &mut state, allow_input_wait) {
            Ok(processed_input) => did_work |= processed_input,
            Err(_) => return 0xfd0b,
        }

        if did_work {
            continue;
        }

        if rt::yield_current().is_err() {
            return 0xfd08;
        }
    }
}

struct SessionState {
    focused_surface: u32,
    surface_count_hint: u32,
    input_source: SessionInputSource,
    input_handle: rt::Handle,
    desktop_handle: rt::Handle,
    output_width: u32,
    output_height: u32,
    pointer_x: i32,
    pointer_y: i32,
    modifiers: u32,
}

fn handle_request(
    request: &RawMessage,
    log_handle: rt::Handle,
    state: &mut SessionState,
) -> rt::Result<()> {
    match request.tag {
        x if x == SessionTag::ListRequest as u32 => {
            if request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(SessionTag::ListReply as u32);
            reply.word_count = 3;
            reply.words[0] = SessionStatus::Ok as u32 as u64;
            reply.words[1] = 1;
            reply.words[2] = SESSION_ID as u64;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == SessionTag::StatusRequest as u32 => {
            if request.word_count < 1 || request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(SessionTag::StatusReply as u32);
            reply.word_count = 5;
            if request.words[0] as u32 != SESSION_ID {
                reply.words[0] = SessionStatus::NotFound as u32 as u64;
            } else {
                reply.words[0] = SessionStatus::Ok as u32 as u64;
                reply.words[1] = SESSION_ID as u64;
                reply.words[2] = state.input_source as u32 as u64;
                reply.words[3] = state.focused_surface as u64;
                reply.words[4] = state.surface_count_hint as u64;
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == SessionTag::FocusRequest as u32 => {
            if request.word_count < 2 || request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let session_id = request.words[0] as u32;
            let surface_id = request.words[1] as u32;
            let mut reply = RawMessage::empty(SessionTag::FocusReply as u32);
            reply.word_count = 2;

            if session_id != SESSION_ID {
                reply.words[0] = SessionStatus::NotFound as u32 as u64;
                reply.words[1] = 0;
            } else {
                state.focused_surface = surface_id;
                if surface_id != 0 {
                    state.surface_count_hint = state.surface_count_hint.max(1);
                }
                reply.words[0] = SessionStatus::Ok as u32 as u64;
                reply.words[1] = state.focused_surface as u64;
                let _ = emit_log(
                    log_handle,
                    LogSeverity::Info,
                    LogEvent::SessionFocusChanged,
                    SESSION_ID as u64,
                    state.focused_surface as u64,
                );
            }

            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        _ => {}
    }

    Ok(())
}

/// Wakeup-driven receive: parks the thread in the kernel until the device IRQ
/// path signals the input source (`notify_input_ready`), then applies a
/// one-shot nonblocking drain fallback so a consumed wakeup never strands
/// coalesced or late arrivals behind a missed edge.
fn await_input_wakeup(state: &mut SessionState) -> rt::Result<Option<rt::InputEventInfo>> {
    match rt::input_source_receive(state.input_handle) {
        Ok(event) => Ok(Some(event)),
        Err(rt::Error::QueueEmpty) => {
            match rt::input_source_receive_nonblocking(state.input_handle) {
                Ok(event) => Ok(Some(event)),
                Err(rt::Error::QueueEmpty) => Ok(None),
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

fn poll_input(
    bootstrap: rt::Handle,
    state: &mut SessionState,
    allow_wait: bool,
) -> rt::Result<bool> {
    let mut processed = false;
    let mut pending_pointer_move = false;
    let mut remaining = MAX_INPUT_EVENTS_PER_TURN;
    let mut wakeup_wait_used = false;
    loop {
        let event = match rt::input_source_receive_nonblocking(state.input_handle) {
            Ok(event) => {
                processed = true;
                event
            }
            Err(rt::Error::QueueEmpty) => {
                if allow_wait && !wakeup_wait_used {
                    wakeup_wait_used = true;
                    match await_input_wakeup(state)? {
                        Some(event) => {
                            processed = true;
                            event
                        }
                        None => {
                            if pending_pointer_move {
                                flush_pointer_move(bootstrap, state)?;
                            }
                            return Ok(processed);
                        }
                    }
                } else {
                    if pending_pointer_move {
                        flush_pointer_move(bootstrap, state)?;
                    }
                    return Ok(processed);
                }
            }
            Err(error) => return Err(error),
        };
        match event.kind {
            x if x == InputEventKind::PointerMotion as u32 => {
                state.pointer_x = scale_input_axis(event.value0, state.output_width);
                state.pointer_y = scale_input_axis(event.value1, state.output_height);
                pending_pointer_move = true;
            }
            x if x == InputEventKind::PointerDelta as u32 => {
                state.pointer_x =
                    clamp_axis(state.pointer_x.saturating_add(event.value0), state.output_width);
                state.pointer_y =
                    clamp_axis(state.pointer_y.saturating_add(event.value1), state.output_height);
                pending_pointer_move = true;
            }
            _ => {
                if pending_pointer_move {
                    flush_pointer_move(bootstrap, state)?;
                    pending_pointer_move = false;
                }
                process_input_event(bootstrap, state, event)?;
            }
        }
        remaining = remaining.saturating_sub(1);
        if remaining == 0 {
            if pending_pointer_move {
                flush_pointer_move(bootstrap, state)?;
            }
            return Ok(processed);
        }
    }
}

fn flush_pointer_move(bootstrap: rt::Handle, state: &mut SessionState) -> rt::Result<()> {
    let Some(desktop_handle) = desktop_handle(bootstrap, state)? else {
        return Ok(());
    };
    tolerate_input_backpressure(rt::desktop_pointer_input_async(
        desktop_handle,
        DesktopInputAction::PointerMove,
        state.pointer_x,
        state.pointer_y,
    ))
}

fn process_input_event(
    bootstrap: rt::Handle,
    state: &mut SessionState,
    event: rt::InputEventInfo,
) -> rt::Result<()> {
    let Some(desktop_handle) = desktop_handle(bootstrap, state)? else {
        return Ok(());
    };

    match event.kind {
        x if x == InputEventKind::PointerButton as u32 => {
            let action = if event.value0 == 0 {
                DesktopInputAction::PointerUp
            } else {
                DesktopInputAction::PointerDown
            };
            if event.code == InputButton::Left as u32 {
                tolerate_input_backpressure(rt::desktop_pointer_input_async(
                    desktop_handle,
                    action,
                    state.pointer_x,
                    state.pointer_y,
                ))?;
            }
        }
        x if x == InputEventKind::PointerScroll as u32 => {
            tolerate_input_backpressure(rt::desktop_pointer_scroll_input_async(
                desktop_handle,
                state.pointer_x,
                state.pointer_y,
                event.value1,
            ))?;
        }
        x if x == InputEventKind::Key as u32 => {
            update_modifier_state(state, event.code, event.value0 != 0);
            let key_action = if event.value0 == 0 {
                DesktopInputAction::KeyUp
            } else {
                DesktopInputAction::KeyDown
            };
            tolerate_input_backpressure(rt::desktop_key_input_async(
                desktop_handle,
                key_action,
                event.code,
                state.modifiers,
            ))?;
            if event.value0 != 0 {
                if state.modifiers & MOD_CTRL == 0 {
                    if let Some(ch) = keycode_to_text(event.code, state.modifiers) {
                    tolerate_input_backpressure(rt::desktop_key_input_async(
                        desktop_handle,
                        DesktopInputAction::TextInput,
                        ch as u32,
                        state.modifiers,
                    ))?;
                    }
                }
            }
        }
        x if x == InputEventKind::PointerMotion as u32
            || x == InputEventKind::PointerDelta as u32 => {}
        _ => {}
    }

    Ok(())
}

fn desktop_handle(bootstrap: rt::Handle, state: &mut SessionState) -> rt::Result<Option<rt::Handle>> {
    if state.desktop_handle != rt::INVALID_HANDLE {
        return Ok(Some(state.desktop_handle));
    }
    match rt::lookup_service(bootstrap, ServiceId::DesktopShell) {
        Ok(handle) => {
            state.desktop_handle = handle;
            Ok(Some(handle))
        }
        Err(rt::Error::NotFound) => Ok(None),
        Err(error) => Err(error),
    }
}

fn lookup_output_size(bootstrap: rt::Handle) -> rt::Result<(u32, u32)> {
    let graphics_handle = rt::lookup_service(bootstrap, ServiceId::Graphics)?;
    let output = rt::graphics_output_status(graphics_handle, 0)?
        .ok_or(rt::Error::NotFound)?;
    let _ = rt::handle_close(graphics_handle);
    Ok((output.width, output.height))
}

fn scale_input_axis(value: i32, limit: u32) -> i32 {
    if limit == 0 {
        return 0;
    }
    let clamped = value.clamp(0, 65_535) as u64;
    ((clamped.saturating_mul((limit.saturating_sub(1)) as u64)) / 65_535) as i32
}

fn clamp_axis(value: i32, limit: u32) -> i32 {
    if limit == 0 {
        return 0;
    }
    value.clamp(0, limit.saturating_sub(1) as i32)
}

fn tolerate_input_backpressure(result: rt::Result<()>) -> rt::Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(rt::Error::Busy | rt::Error::CapacityExceeded) => Ok(()),
        Err(error) => Err(error),
    }
}

fn update_modifier_state(state: &mut SessionState, key_code: u32, pressed: bool) {
    let bit = match key_code {
        KEY_LEFT_SHIFT | KEY_RIGHT_SHIFT => MOD_SHIFT,
        KEY_LEFT_ALT | KEY_RIGHT_ALT => MOD_ALT,
        KEY_LEFT_CTRL | KEY_RIGHT_CTRL => MOD_CTRL,
        _ => 0,
    };
    if bit == 0 {
        return;
    }
    if pressed {
        state.modifiers |= bit;
    } else {
        state.modifiers &= !bit;
    }
}

fn keycode_to_text(key_code: u32, modifiers: u32) -> Option<char> {
    let shift = modifiers & MOD_SHIFT != 0;
    match key_code {
        2 => Some(if shift { '!' } else { '1' }),
        3 => Some(if shift { '@' } else { '2' }),
        4 => Some(if shift { '#' } else { '3' }),
        5 => Some(if shift { '$' } else { '4' }),
        6 => Some(if shift { '%' } else { '5' }),
        7 => Some(if shift { '^' } else { '6' }),
        8 => Some(if shift { '&' } else { '7' }),
        9 => Some(if shift { '*' } else { '8' }),
        10 => Some(if shift { '(' } else { '9' }),
        11 => Some(if shift { ')' } else { '0' }),
        12 => Some(if shift { '_' } else { '-' }),
        13 => Some(if shift { '+' } else { '=' }),
        15 => Some('\t'),
        16 => Some(if shift { 'Q' } else { 'q' }),
        17 => Some(if shift { 'W' } else { 'w' }),
        18 => Some(if shift { 'E' } else { 'e' }),
        19 => Some(if shift { 'R' } else { 'r' }),
        20 => Some(if shift { 'T' } else { 't' }),
        21 => Some(if shift { 'Y' } else { 'y' }),
        22 => Some(if shift { 'U' } else { 'u' }),
        23 => Some(if shift { 'I' } else { 'i' }),
        24 => Some(if shift { 'O' } else { 'o' }),
        25 => Some(if shift { 'P' } else { 'p' }),
        26 => Some(if shift { '{' } else { '[' }),
        27 => Some(if shift { '}' } else { ']' }),
        28 => Some('\n'),
        30 => Some(if shift { 'A' } else { 'a' }),
        31 => Some(if shift { 'S' } else { 's' }),
        32 => Some(if shift { 'D' } else { 'd' }),
        33 => Some(if shift { 'F' } else { 'f' }),
        34 => Some(if shift { 'G' } else { 'g' }),
        35 => Some(if shift { 'H' } else { 'h' }),
        36 => Some(if shift { 'J' } else { 'j' }),
        37 => Some(if shift { 'K' } else { 'k' }),
        38 => Some(if shift { 'L' } else { 'l' }),
        39 => Some(if shift { ':' } else { ';' }),
        40 => Some(if shift { '"' } else { '\'' }),
        41 => Some(if shift { '~' } else { '`' }),
        43 => Some(if shift { '|' } else { '\\' }),
        44 => Some(if shift { 'Z' } else { 'z' }),
        45 => Some(if shift { 'X' } else { 'x' }),
        46 => Some(if shift { 'C' } else { 'c' }),
        47 => Some(if shift { 'V' } else { 'v' }),
        48 => Some(if shift { 'B' } else { 'b' }),
        49 => Some(if shift { 'N' } else { 'n' }),
        50 => Some(if shift { 'M' } else { 'm' }),
        51 => Some(if shift { '<' } else { ',' }),
        52 => Some(if shift { '>' } else { '.' }),
        53 => Some(if shift { '?' } else { '/' }),
        57 => Some(' '),
        _ => None,
    }
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

fn emit_log(
    log_handle: rt::Handle,
    severity: LogSeverity,
    event: LogEvent,
    arg0: u64,
    arg1: u64,
) -> rt::Result<()> {
    rt::send_log_record(
        log_handle,
        ServiceId::Session,
        severity,
        LogDomain::Session,
        event,
        arg0,
        arg1,
    )
}
