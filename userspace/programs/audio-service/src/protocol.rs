use serviceos_userspace_runtime as rt;
use rt::{
    AudioEndpointStatusInfo, AudioStatus, AudioStreamDirection, AudioStreamState, AudioTag,
    LogEvent, LogSeverity, RawMessage,
};

use crate::{
    consts::MAX_AUDIO_STREAMS,
    types::StreamSlot,
    util::{emit_log, ticks_from_ms},
};

pub(crate) fn handle_public_request(
    request: &RawMessage,
    log_handle: rt::Handle,
    endpoint: AudioEndpointStatusInfo,
    streams: &mut [StreamSlot; MAX_AUDIO_STREAMS],
) -> rt::Result<()> {
    match request.tag {
        x if x == AudioTag::EndpointListRequest as u32 => {
            if request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(AudioTag::EndpointListReply as u32);
            reply.word_count = 2;
            reply.words[0] = AudioStatus::Ok as u32 as u64;
            reply.words[1] = 1;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == AudioTag::EndpointStatusRequest as u32 => {
            if request.handle_count < 1 || request.word_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(AudioTag::EndpointStatusReply as u32);
            reply.word_count = 12;
            if request.words[0] != 0 {
                reply.words[0] = AudioStatus::NotFound as u32 as u64;
            } else {
                reply.words[0] = AudioStatus::Ok as u32 as u64;
                reply.words[1] = endpoint.index as u64;
                reply.words[2] = endpoint.backend as u32 as u64;
                reply.words[3] = endpoint.direction as u32 as u64;
                reply.words[4] = endpoint.state as u32 as u64;
                reply.words[5] = endpoint.capabilities as u64;
                reply.words[6] = endpoint.nominal_rate_hz as u64;
                reply.words[7] = endpoint.channels as u64;
                reply.words[8] = endpoint.min_frequency_hz as u64;
                reply.words[9] = endpoint.max_frequency_hz as u64;
                reply.words[10] = endpoint.current_frequency_hz as u64;
                reply.words[11] = endpoint.play_count;
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == AudioTag::StreamOpenRequest as u32 => {
            if request.handle_count < 1 || request.word_count < 2 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(AudioTag::StreamOpenReply as u32);
            reply.word_count = 1;
            match allocate_stream(streams, request.words[1] as u32) {
                Ok((slot, client_handle)) => {
                    reply.words[0] = AudioStatus::Ok as u32 as u64;
                    reply.word_count = 2;
                    reply.words[1] = slot as u64;
                    reply.handle_count = 1;
                    reply.handles[0] = client_handle;
                    let _ = emit_log(
                        log_handle,
                        LogSeverity::Info,
                        LogEvent::AudioStreamOpened,
                        slot as u64,
                        request.words[1],
                    );
                }
                Err(error) => {
                    reply.words[0] = match error {
                        rt::Error::CapacityExceeded => AudioStatus::CapacityExceeded as u32 as u64,
                        rt::Error::PermissionDenied => AudioStatus::Denied as u32 as u64,
                        rt::Error::Unsupported => AudioStatus::Unsupported as u32 as u64,
                        _ => AudioStatus::Busy as u32 as u64,
                    };
                }
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == AudioTag::StreamListRequest as u32 => {
            if request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let start = if request.word_count > 0 {
                request.words[0] as usize
            } else {
                0
            };
            let mut reply = RawMessage::empty(AudioTag::StreamListReply as u32);
            reply.words[0] = AudioStatus::Ok as u32 as u64;
            reply.word_count = 3;
            let mut count = 0usize;
            let mut next = usize::MAX;
            let mut cursor = 3usize;
            for (slot, stream) in streams.iter().enumerate().skip(start) {
                if !stream.active {
                    continue;
                }
                if cursor + 6 > reply.words.len() {
                    next = slot;
                    break;
                }
                reply.words[cursor] = slot as u64;
                reply.words[cursor + 1] = AudioStreamDirection::Playback as u32 as u64;
                reply.words[cursor + 2] = stream.state as u32 as u64;
                reply.words[cursor + 3] = stream.session_id as u64;
                reply.words[cursor + 4] = stream.endpoint_index as u64;
                reply.words[cursor + 5] = stream.frequency_hz as u64;
                count += 1;
                cursor += 6;
            }
            reply.word_count = cursor as u32;
            reply.words[1] = count as u64;
            reply.words[2] = next as u64;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn handle_stream_request(
    slot_index: usize,
    request: &RawMessage,
    audio_handle: rt::Handle,
    log_handle: rt::Handle,
    streams: &mut [StreamSlot; MAX_AUDIO_STREAMS],
) -> rt::Result<()> {
    match request.tag {
        x if x == AudioTag::StreamStatusRequest as u32 => {
            if request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let stream = streams[slot_index];
            let mut reply = RawMessage::empty(AudioTag::StreamStatusReply as u32);
            reply.word_count = 8;
            reply.words[0] = AudioStatus::Ok as u32 as u64;
            reply.words[1] = slot_index as u64;
            reply.words[2] = AudioStreamDirection::Playback as u32 as u64;
            reply.words[3] = stream.state as u32 as u64;
            reply.words[4] = stream.session_id as u64;
            reply.words[5] = stream.endpoint_index as u64;
            reply.words[6] = stream.frequency_hz as u64;
            reply.words[7] = stream.until_tick.saturating_sub(rt::monotonic_now().unwrap_or(0));
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == AudioTag::StreamPlayToneRequest as u32 => {
            if request.handle_count < 1 || request.word_count < 2 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(AudioTag::StreamPlayToneReply as u32);
            reply.word_count = 1;
            let frequency_hz = request.words[0] as u32;
            let duration_ms = request.words[1] as u32;
            let duration_ticks = ticks_from_ms(duration_ms);
            let result = rt::kernel_audio_endpoint_play_tone(audio_handle, frequency_hz, duration_ticks);
            match result {
                Ok(()) => {
                    let until_tick = rt::monotonic_now()
                        .unwrap_or(0)
                        .saturating_add(duration_ticks as u64);
                    for (index, stream) in streams.iter_mut().enumerate() {
                        if !stream.active {
                            continue;
                        }
                        if index == slot_index {
                            stream.state = AudioStreamState::Active;
                            stream.frequency_hz = frequency_hz;
                            stream.until_tick = until_tick;
                        } else if stream.state == AudioStreamState::Active {
                            stream.state = AudioStreamState::Idle;
                            stream.frequency_hz = 0;
                            stream.until_tick = 0;
                            let _ = emit_log(
                                log_handle,
                                LogSeverity::Info,
                                LogEvent::AudioStreamStopped,
                                index as u64,
                                0,
                            );
                        }
                    }
                    reply.words[0] = AudioStatus::Ok as u32 as u64;
                    let _ = emit_log(
                        log_handle,
                        LogSeverity::Info,
                        LogEvent::AudioStreamStarted,
                        slot_index as u64,
                        ((frequency_hz as u64) << 32) | duration_ms as u64,
                    );
                }
                Err(rt::Error::Busy) => reply.words[0] = AudioStatus::Busy as u32 as u64,
                Err(rt::Error::Unsupported) => {
                    reply.words[0] = AudioStatus::Unsupported as u32 as u64
                }
                Err(rt::Error::PermissionDenied) => {
                    reply.words[0] = AudioStatus::Denied as u32 as u64
                }
                Err(rt::Error::CapacityExceeded) => {
                    reply.words[0] = AudioStatus::CapacityExceeded as u32 as u64
                }
                Err(_) => reply.words[0] = AudioStatus::Busy as u32 as u64,
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == AudioTag::StreamCloseRequest as u32 => {
            if request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(AudioTag::StreamCloseReply as u32);
            reply.word_count = 1;
            if streams[slot_index].state == AudioStreamState::Active {
                let _ = rt::kernel_audio_endpoint_stop(audio_handle);
                let _ = emit_log(
                    log_handle,
                    LogSeverity::Info,
                    LogEvent::AudioStreamStopped,
                    slot_index as u64,
                    0,
                );
            }
            close_stream_slot(streams, slot_index);
            reply.words[0] = AudioStatus::Closed as u32 as u64;
            let _ = emit_log(
                log_handle,
                LogSeverity::Info,
                LogEvent::AudioStreamClosed,
                slot_index as u64,
                0,
            );
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        _ => {}
    }
    Ok(())
}

fn allocate_stream(
    streams: &mut [StreamSlot; MAX_AUDIO_STREAMS],
    session_id: u32,
) -> rt::Result<(usize, rt::Handle)> {
    let Some(slot_index) = (0..streams.len()).find(|index| !streams[*index].active) else {
        return Err(rt::Error::CapacityExceeded);
    };
    let pair = rt::channel_create()?;
    streams[slot_index] = StreamSlot {
        active: true,
        control_handle: pair.first,
        session_id,
        endpoint_index: 0,
        frequency_hz: 0,
        until_tick: 0,
        state: AudioStreamState::Idle,
    };
    Ok((slot_index, pair.second))
}

pub(crate) fn close_stream_slot(streams: &mut [StreamSlot; MAX_AUDIO_STREAMS], slot_index: usize) {
    let stream = &mut streams[slot_index];
    if stream.control_handle != rt::INVALID_HANDLE {
        let _ = rt::handle_close(stream.control_handle);
    }
    *stream = StreamSlot::empty();
}
