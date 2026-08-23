use rt::{
    AudioEndpointStatusInfo, AudioStreamState, ControlTag, LifecycleEvent, LogDomain, LogEvent,
    LogSeverity, RawMessage, ServiceId,
};
use serviceos_userspace_runtime as rt;

pub(crate) fn poll_lifecycle(bootstrap: rt::Handle) -> rt::Result<bool> {
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

pub(crate) fn lifecycle_event_from_word(value: u64) -> LifecycleEvent {
    match value as u32 {
        x if x == LifecycleEvent::Starting as u32 => LifecycleEvent::Starting,
        x if x == LifecycleEvent::Ready as u32 => LifecycleEvent::Ready,
        x if x == LifecycleEvent::Failed as u32 => LifecycleEvent::Failed,
        x if x == LifecycleEvent::Stopped as u32 => LifecycleEvent::Stopped,
        _ => LifecycleEvent::Restarting,
    }
}

pub(crate) fn emit_log(
    log_handle: rt::Handle,
    severity: LogSeverity,
    event: LogEvent,
    arg0: u64,
    arg1: u64,
) -> rt::Result<()> {
    rt::send_log_record(
        log_handle,
        ServiceId::Audio,
        severity,
        LogDomain::Audio,
        event,
        arg0,
        arg1,
    )
}

pub(crate) fn ticks_from_ms(duration_ms: u32) -> u32 {
    duration_ms.div_ceil(10).max(1)
}

pub(crate) fn update_stream_expiry(
    endpoint: AudioEndpointStatusInfo,
    streams: &mut [crate::types::StreamSlot],
) -> Option<usize> {
    let now = rt::monotonic_now().unwrap_or(0);
    let mut stopped = None;
    let endpoint_active = endpoint.state == rt::AudioEndpointState::Active;
    for (index, slot) in streams.iter_mut().enumerate() {
        if !slot.active {
            continue;
        }
        // PCM streams are driven by the mixer, not by speaker tone expiry.
        if slot.pcm_configured {
            continue;
        }
        let expired = slot.until_tick != 0 && now >= slot.until_tick;
        if !endpoint_active || expired {
            if slot.state == AudioStreamState::Active {
                stopped = Some(index);
            }
            slot.state = AudioStreamState::Idle;
            slot.frequency_hz = 0;
            slot.until_tick = 0;
        }
    }
    stopped
}
