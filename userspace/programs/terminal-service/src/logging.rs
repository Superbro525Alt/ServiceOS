use serviceos_userspace_runtime as rt;
use rt::{ControlTag, LifecycleEvent, LogDomain, LogEvent, LogSeverity, RawMessage, ServiceId};

pub(crate) fn poll_lifecycle(bootstrap: rt::Handle) -> rt::Result<bool> {
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

pub(crate) fn emit_terminal_log(
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
