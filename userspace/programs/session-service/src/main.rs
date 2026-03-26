#![no_std]
#![no_main]

use serviceos_userspace_runtime as rt;
use rt::{
    ControlTag, LifecycleEvent, LogDomain, LogEvent, LogSeverity, RawMessage, ServiceId,
    SessionInputSource, SessionStatus, SessionTag,
};

const SESSION_ID: u32 = 1;

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

    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return 0xfd03,
    };
    if rt::register_service(bootstrap, ServiceId::Session, public.second).is_err() {
        return 0xfd04;
    }
    let _ = rt::handle_close(public.second);

    let mut state = SessionState {
        focused_surface: 0,
        surface_count_hint: 0,
    };
    let _ = emit_log(
        log_handle,
        LogSeverity::Info,
        LogEvent::SessionReady,
        SESSION_ID as u64,
        0,
    );

    loop {
        match poll_lifecycle(bootstrap) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(_) => return 0xfd05,
        }

        let mut request = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut request) {
            Ok(()) => {
                if handle_request(&request, log_handle, &mut state).is_err() {
                    return 0xfd06;
                }
            }
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => return 0xfd07,
        }

        if rt::yield_current().is_err() {
            return 0xfd08;
        }
    }
}

struct SessionState {
    focused_surface: u32,
    surface_count_hint: u32,
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
