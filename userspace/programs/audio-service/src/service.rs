use serviceos_userspace_runtime as rt;
use rt::{
    AudioEndpointState, ControlTag, LogEvent, LogSeverity, RawMessage, ServiceId,
};

use crate::{
    consts::MAX_AUDIO_STREAMS,
    protocol::{close_stream_slot, handle_public_request, handle_stream_request},
    types::StreamSlot,
    util::{emit_log, poll_lifecycle, update_stream_expiry},
};

pub(crate) fn run() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xfa01;
    }
    if startup.tag != ControlTag::Startup as u32 || startup.handle_count < 2 {
        return 0xfa02;
    }

    let audio_handle = startup.handles[0];
    let log_handle = startup.handles[1];

    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return 0xfa03,
    };
    if rt::register_service(bootstrap, ServiceId::Audio, public.second).is_err() {
        return 0xfa04;
    }
    let _ = rt::handle_close(public.second);

    let endpoint = match rt::kernel_audio_endpoint_status(audio_handle) {
        Ok(endpoint) => endpoint,
        Err(_) => return 0xfa05,
    };
    let _ = emit_log(
        log_handle,
        LogSeverity::Info,
        LogEvent::AudioEndpointReady,
        endpoint.backend as u32 as u64,
        endpoint.capabilities as u64,
    );

    let mut streams = [StreamSlot::empty(); MAX_AUDIO_STREAMS];

    loop {
        if poll_lifecycle(bootstrap).unwrap_or(false) {
            for slot in 0..streams.len() {
                if streams[slot].active {
                    close_stream_slot(&mut streams, slot);
                }
            }
            let _ = rt::kernel_audio_endpoint_stop(audio_handle);
            return 0;
        }

        let endpoint = match rt::kernel_audio_endpoint_status(audio_handle) {
            Ok(endpoint) => endpoint,
            Err(_) => return 0xfa06,
        };
        if let Some(stopped_slot) = update_stream_expiry(endpoint, &mut streams)
            && (endpoint.state == AudioEndpointState::Idle || streams[stopped_slot].frequency_hz == 0)
        {
            let _ = emit_log(
                log_handle,
                LogSeverity::Info,
                LogEvent::AudioStreamStopped,
                stopped_slot as u64,
                0,
            );
        }

        let mut had_work = false;
        let mut request = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut request) {
            Ok(()) => {
                had_work = true;
                if handle_public_request(&request, log_handle, endpoint, &mut streams).is_err() {
                    return 0xfa07;
                }
            }
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => return 0xfa08,
        }

        for slot in 0..streams.len() {
            if !streams[slot].active {
                continue;
            }
            let mut request = RawMessage::empty(0);
            match rt::channel_receive_nonblocking(streams[slot].control_handle, &mut request) {
                Ok(()) => {
                    had_work = true;
                    if handle_stream_request(slot, &request, audio_handle, log_handle, &mut streams).is_err() {
                        return 0xfa09;
                    }
                }
                Err(rt::Error::QueueEmpty) => {}
                Err(_) => {
                    close_stream_slot(&mut streams, slot);
                }
            }
        }

        if !had_work && rt::yield_current().is_err() {
            return 0xfa0a;
        }
    }
}
