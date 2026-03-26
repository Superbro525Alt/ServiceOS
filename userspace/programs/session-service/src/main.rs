#![no_std]
#![no_main]

use serviceos_userspace_runtime as rt;
use rt::{
    ControlTag, LifecycleEvent, LogDomain, LogEvent, LogSeverity, RawMessage, ServiceId,
    SessionInputSource, SessionStatus, SessionTag,
};

const SESSION_ID: u32 = 1;
const BACKGROUND_IDLE_RGB: u32 = 0x1b2740;
const BACKGROUND_FOCUSED_RGB: u32 = 0x29456e;
const PANEL_IDLE_RGB: u32 = 0x5a6372;
const PANEL_FOCUSED_RGB: u32 = 0x7cc6ff;

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xfd01;
    }
    if startup.tag != ControlTag::Startup as u32 || startup.handle_count < 1 {
        return 0xfd02;
    }

    let log_handle = startup.handles[0];
    let graphics_handle = match rt::lookup_service(bootstrap, ServiceId::Graphics) {
        Ok(handle) => handle,
        Err(_) => return 0xfd03,
    };
    let output = match rt::graphics_output_status(graphics_handle, 0) {
        Ok(Some(output)) => output,
        _ => return 0xfd04,
    };

    let (background_id, background_handle) = match rt::graphics_surface_create(
        graphics_handle,
        SESSION_ID,
        0,
        0,
        output.width,
        output.height,
        0,
        BACKGROUND_IDLE_RGB,
        true,
    ) {
        Ok(surface) => surface,
        Err(_) => return 0xfd05,
    };
    let (panel_id, panel_handle) = match rt::graphics_surface_create(
        graphics_handle,
        SESSION_ID,
        (output.width / 4) as i32,
        (output.height / 3) as i32,
        output.width / 2,
        output.height / 3,
        1,
        PANEL_FOCUSED_RGB,
        true,
    ) {
        Ok(surface) => surface,
        Err(_) => return 0xfd06,
    };
    let _ = rt::handle_close(graphics_handle);

    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return 0xfd07,
    };
    if rt::register_service(bootstrap, ServiceId::Session, public.second).is_err() {
        return 0xfd08;
    }
    let _ = rt::handle_close(public.second);

    let mut state = SessionState {
        focused_surface: panel_id,
        background_id,
        panel_id,
        background_handle,
        panel_handle,
    };
    let _ = emit_log(
        log_handle,
        LogSeverity::Info,
        LogEvent::SessionReady,
        SESSION_ID as u64,
        state.focused_surface as u64,
    );

    loop {
        match poll_lifecycle(bootstrap) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(_) => return 0xfd09,
        }

        let mut request = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut request) {
            Ok(()) => {
                if handle_request(&request, log_handle, &mut state).is_err() {
                    return 0xfd0a;
                }
            }
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => return 0xfd0b,
        }

        if rt::yield_current().is_err() {
            return 0xfd0c;
        }
    }
}

struct SessionState {
    focused_surface: u32,
    background_id: u32,
    panel_id: u32,
    background_handle: rt::Handle,
    panel_handle: rt::Handle,
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
                reply.words[2] = SessionInputSource::ServiceControl as u32 as u64;
                reply.words[3] = state.focused_surface as u64;
                reply.words[4] = 2;
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
            } else if apply_focus(state, surface_id).is_err() {
                reply.words[0] = SessionStatus::NotFound as u32 as u64;
                reply.words[1] = state.focused_surface as u64;
            } else {
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

fn apply_focus(state: &mut SessionState, surface_id: u32) -> rt::Result<()> {
    match surface_id {
        id if id == state.background_id => {
            rt::surface_set_fill(state.background_handle, BACKGROUND_FOCUSED_RGB)?;
            rt::surface_set_fill(state.panel_handle, PANEL_IDLE_RGB)?;
            state.focused_surface = state.background_id;
            Ok(())
        }
        id if id == state.panel_id => {
            rt::surface_set_fill(state.background_handle, BACKGROUND_IDLE_RGB)?;
            rt::surface_set_fill(state.panel_handle, PANEL_FOCUSED_RGB)?;
            state.focused_surface = state.panel_id;
            Ok(())
        }
        _ => Err(rt::Error::NotFound),
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
