use rt::{
    AudioEndpointStatusInfo, AudioStatus, AudioStreamDirection, AudioStreamState, AudioTag,
    LogEvent, LogSeverity, RawMessage,
};
use serviceos_abi::{
    AudioSampleFormat, PcmNullSink, PcmStreamState, PCM_RING_FRAMES, SINK_RATE_HZ,
    audio_stream_write_flag, pcm_resampled_len,
};
use serviceos_userspace_runtime as rt;

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
    pcm: &mut [PcmStreamState; MAX_AUDIO_STREAMS],
    sink: &mut PcmNullSink,
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
            reply.word_count = 14;
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
                // Service-local mixed-PCM null-sink counters (extension words;
                // older clients only parse the first 12).
                reply.words[12] = sink.frames_mixed;
                reply.words[13] = sink.checksum;
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == AudioTag::EndpointVolumeSetRequest as u32 => {
            if request.handle_count < 1 || request.word_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let volume = (request.words[0] as u64).min(100) as u8;
            let muted = request.word_count > 1 && request.words[1] != 0;
            sink.master_volume = volume;
            sink.master_muted = muted;
            let mut reply = RawMessage::empty(AudioTag::EndpointVolumeSetReply as u32);
            reply.word_count = 3;
            reply.words[0] = AudioStatus::Ok as u32 as u64;
            reply.words[1] = volume as u64;
            reply.words[2] = muted as u64;
            let _ = emit_log(
                log_handle,
                LogSeverity::Info,
                LogEvent::AudioEndpointReady,
                volume as u64,
                muted as u64,
            );
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
            match allocate_stream(streams, pcm, request.words[1] as u32) {
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
    pcm: &mut [PcmStreamState; MAX_AUDIO_STREAMS],
    sink: &mut PcmNullSink,
) -> rt::Result<()> {
    match request.tag {
        x if x == AudioTag::StreamStatusRequest as u32 => {
            if request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let stream = streams[slot_index];
            let state = &pcm[slot_index];
            let mut reply = RawMessage::empty(AudioTag::StreamStatusReply as u32);
            reply.word_count = 14;
            reply.words[0] = AudioStatus::Ok as u32 as u64;
            reply.words[1] = slot_index as u64;
            reply.words[2] = AudioStreamDirection::Playback as u32 as u64;
            reply.words[3] = stream.state as u32 as u64;
            reply.words[4] = stream.session_id as u64;
            reply.words[5] = stream.endpoint_index as u64;
            reply.words[6] = stream.frequency_hz as u64;
            reply.words[7] = stream
                .until_tick
                .saturating_sub(rt::monotonic_now().unwrap_or(0));
            // PCM extension words.
            reply.words[8] = state.rate_hz as u64;
            reply.words[9] = state.channels as u64;
            reply.words[10] = state.format as u32 as u64;
            reply.words[11] = state.volume as u64;
            reply.words[12] = state.muted as u64;
            reply.words[13] = state.frames_written;
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
            let result =
                rt::kernel_audio_endpoint_play_tone(audio_handle, frequency_hz, duration_ticks);
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
                        } else if stream.state == AudioStreamState::Active && !stream.pcm_configured
                        {
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
        x if x == AudioTag::StreamConfigureRequest as u32 => {
            if request.handle_count < 1 || request.word_count < 3 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(AudioTag::StreamConfigureReply as u32);
            reply.word_count = 6;
            let format = match request.words[0] {
                x if x == AudioSampleFormat::U8 as u32 as u64 => Some(AudioSampleFormat::U8),
                x if x == AudioSampleFormat::S16Le as u32 as u64 => Some(AudioSampleFormat::S16Le),
                x if x == AudioSampleFormat::S32Le as u32 as u64 => Some(AudioSampleFormat::S32Le),
                x if x == AudioSampleFormat::F32Le as u32 as u64 => Some(AudioSampleFormat::F32Le),
                _ => None,
            };
            match format.and_then(|format| {
                PcmStreamState::negotiate(format, request.words[1] as u32, request.words[2] as u32)
            }) {
                Some((format, rate, channels)) => {
                    streams[slot_index].pcm_configured = true;
                    pcm[slot_index].active = true;
                    pcm[slot_index].apply_config(format, rate, channels);
                    reply.words[0] = AudioStatus::Ok as u32 as u64;
                    reply.words[1] = format as u32 as u64;
                    reply.words[2] = rate as u64;
                    reply.words[3] = channels as u64;
                    reply.words[4] = SINK_RATE_HZ as u64;
                    reply.words[5] = PCM_RING_FRAMES as u64;
                }
                None => {
                    reply.words[0] = AudioStatus::Unsupported as u32 as u64;
                }
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == AudioTag::StreamWriteRequest as u32 => {
            if request.handle_count < 1 || request.word_count < 2 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(AudioTag::StreamWriteReply as u32);
            reply.word_count = 4;
            if !streams[slot_index].pcm_configured {
                reply.words[0] = AudioStatus::Unsupported as u32 as u64;
            } else {
                let frame_count = request.words[0] as usize;
                let flags = request.words[1];
                let blocking = flags & audio_stream_write_flag::BLOCKING != 0;
                let sample_count = frame_count * pcm[slot_index].channels as usize;
                let needed = pcm_resampled_len(frame_count, pcm[slot_index].rate_hz, SINK_RATE_HZ);
                // Blocking writes may synchronously drain queued frames to the
                // sink to make room; nonblocking writes fail with Busy instead.
                if blocking && pcm[slot_index].ring.free() < needed {
                    sink.mix_batch(pcm, PCM_RING_FRAMES.max(needed));
                }
                if frame_count > 0 {
                    match pcm[slot_index].ingest_chunk(&request.words[2..], sample_count) {
                        Some(queued) => {
                            streams[slot_index].state = AudioStreamState::Active;
                            reply.words[0] = AudioStatus::Ok as u32 as u64;
                            reply.words[1] = queued as u64;
                            reply.words[2] = pcm[slot_index].ring.free() as u64;
                            reply.words[3] = pcm[slot_index].frames_written;
                        }
                        None => {
                            reply.words[0] = AudioStatus::Busy as u32 as u64;
                            reply.words[2] = pcm[slot_index].ring.free() as u64;
                            reply.words[3] = pcm[slot_index].frames_written;
                        }
                    }
                } else {
                    reply.words[0] = AudioStatus::Ok as u32 as u64;
                    reply.words[1] = 0;
                    reply.words[2] = pcm[slot_index].ring.free() as u64;
                    reply.words[3] = pcm[slot_index].frames_written;
                }
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == AudioTag::StreamDrainRequest as u32 => {
            if request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(AudioTag::StreamDrainReply as u32);
            reply.word_count = 4;
            if !streams[slot_index].pcm_configured {
                reply.words[0] = AudioStatus::Unsupported as u32 as u64;
            } else {
                sink.mix_until_empty(pcm);
                streams[slot_index].state = AudioStreamState::Idle;
                reply.words[0] = AudioStatus::Ok as u32 as u64;
                reply.words[1] = pcm[slot_index].frames_written;
                reply.words[2] = pcm[slot_index].checksum;
                reply.words[3] = pcm[slot_index].ring.len() as u64;
                let _ = emit_log(
                    log_handle,
                    LogSeverity::Info,
                    LogEvent::AudioStreamStopped,
                    slot_index as u64,
                    pcm[slot_index].checksum,
                );
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == AudioTag::StreamSetVolumeRequest as u32 => {
            if request.handle_count < 1 || request.word_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let volume = (request.words[0] as u64).min(100) as u8;
            let muted = request.word_count > 1 && request.words[1] != 0;
            pcm[slot_index].volume = volume;
            pcm[slot_index].muted = muted;
            let mut reply = RawMessage::empty(AudioTag::StreamSetVolumeReply as u32);
            reply.word_count = 3;
            reply.words[0] = AudioStatus::Ok as u32 as u64;
            reply.words[1] = volume as u64;
            reply.words[2] = muted as u64;
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
                if !streams[slot_index].pcm_configured {
                    let _ = rt::kernel_audio_endpoint_stop(audio_handle);
                }
                let _ = emit_log(
                    log_handle,
                    LogSeverity::Info,
                    LogEvent::AudioStreamStopped,
                    slot_index as u64,
                    0,
                );
            }
            close_stream_slot(streams, pcm, slot_index);
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
    pcm: &mut [PcmStreamState; MAX_AUDIO_STREAMS],
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
        pcm_configured: false,
    };
    pcm[slot_index].reset();
    Ok((slot_index, pair.second))
}

pub(crate) fn close_stream_slot(
    streams: &mut [StreamSlot; MAX_AUDIO_STREAMS],
    pcm: &mut [PcmStreamState; MAX_AUDIO_STREAMS],
    slot_index: usize,
) {
    let stream = &mut streams[slot_index];
    if stream.control_handle != rt::INVALID_HANDLE {
        let _ = rt::handle_close(stream.control_handle);
    }
    *stream = StreamSlot::empty();
    pcm[slot_index].reset();
}
