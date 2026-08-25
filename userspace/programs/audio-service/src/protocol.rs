use rt::{
    AudioEndpointStatusInfo, AudioStatus, AudioStreamState, AudioTag, LogEvent, LogSeverity,
    RawMessage,
};
use serviceos_abi::{
    AudioSampleFormat, AudioStreamDirection, CHECKSUM_SEED, IPC_MAX_WORDS, PCM_RING_FRAMES,
    PcmNullSink, PcmStreamState, SINK_RATE_HZ, audio_stream_read_flag, audio_stream_write_flag,
    capture_checksum_silence, capture_frame_tick, capture_frames_due, capture_pack_silence,
    pcm_resampled_len, pcm_samples_per_word,
};
use serviceos_userspace_runtime as rt;

use crate::{
    consts::{
        CAPTURE_BLOCK_TICKS, CAPTURE_MAX_READ_FRAMES, CAPTURE_REPLY_HEADER_WORDS,
        MAX_AUDIO_STREAMS,
    },
    types::{CaptureStreamState, StreamSlot},
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
            // Generic opens are playback-only; capture opens go through
            // the dedicated CaptureOpen contract above.
            if request.words[0] == AudioStreamDirection::Capture as u32 as u64 {
                reply.words[0] = AudioStatus::Unsupported as u32 as u64;
                let _ = rt::channel_send(reply_handle, &reply);
                let _ = rt::handle_close(reply_handle);
                return Ok(());
            }
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
                reply.words[cursor + 1] = stream.direction as u32 as u64;
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
        x if x == AudioTag::CaptureOpenRequest as u32 => {
            // Capture open carries inline format negotiation, mirroring
            // the playback StreamConfigure contract.
            if request.handle_count < 1 || request.word_count < 3 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(AudioTag::CaptureOpenReply as u32);
            reply.word_count = 5;
            let format = match request.words[0] {
                x if x == AudioSampleFormat::U8 as u32 as u64 => Some(AudioSampleFormat::U8),
                x if x == AudioSampleFormat::S16Le as u32 as u64 => Some(AudioSampleFormat::S16Le),
                x if x == AudioSampleFormat::S32Le as u32 as u64 => Some(AudioSampleFormat::S32Le),
                x if x == AudioSampleFormat::F32Le as u32 as u64 => Some(AudioSampleFormat::F32Le),
                _ => None,
            };
            let session_id = if request.word_count > 3 {
                request.words[3] as u32
            } else {
                0
            };
            let negotiated = format.and_then(|format| {
                PcmStreamState::negotiate(format, request.words[1] as u32, request.words[2] as u32)
            });
            match negotiated {
                Some((format, rate, channels)) => {
                    match allocate_capture_stream(
                        streams,
                        pcm,
                        session_id,
                        format,
                        rate,
                        channels,
                    ) {
                        Ok((slot, client_handle, capture)) => {
                            reply.words[0] = AudioStatus::Ok as u32 as u64;
                            reply.words[1] = slot as u64;
                            reply.words[2] = capture.format as u32 as u64;
                            reply.words[3] = capture.rate_hz as u64;
                            reply.words[4] = capture.channels as u64;
                            reply.handle_count = 1;
                            reply.handles[0] = client_handle;
                            let _ = emit_log(
                                log_handle,
                                LogSeverity::Info,
                                LogEvent::AudioStreamOpened,
                                slot as u64,
                                capture.rate_hz as u64,
                            );
                        }
                        Err(error) => {
                            reply.words[0] = match error {
                                rt::Error::CapacityExceeded => {
                                    AudioStatus::CapacityExceeded as u32 as u64
                                }
                                rt::Error::PermissionDenied => AudioStatus::Denied as u32 as u64,
                                rt::Error::Unsupported => AudioStatus::Unsupported as u32 as u64,
                                _ => AudioStatus::Busy as u32 as u64,
                            };
                        }
                    }
                }
                None => {
                    reply.words[0] = AudioStatus::Unsupported as u32 as u64;
                }
            }
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
    // Capture slots speak the capture read contract; playback-only tags
    // are answered Unsupported with the mirrored reply tag.
    if streams[slot_index].capture.is_some() {
        return handle_capture_stream_request(slot_index, request, log_handle, streams, pcm);
    }
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
            reply.words[2] = stream.direction as u32 as u64;
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
            reply.words[13] = match streams[slot_index].capture {
                Some(capture) => capture.frames_produced,
                None => state.frames_written,
            };
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
        direction: AudioStreamDirection::Playback,
        capture: None,
    };
    pcm[slot_index].reset();
    Ok((slot_index, pair.second))
}

/// Allocate a slot for a null-capture stream with the negotiated
/// configuration; the pacing clock starts now so readers only ever see
/// frames that wall-clock time justifies.
fn allocate_capture_stream(
    streams: &mut [StreamSlot; MAX_AUDIO_STREAMS],
    pcm: &mut [PcmStreamState; MAX_AUDIO_STREAMS],
    session_id: u32,
    format: AudioSampleFormat,
    rate_hz: u32,
    channels: u32,
) -> rt::Result<(usize, rt::Handle, CaptureStreamState)> {
    let Some(slot_index) = (0..streams.len()).find(|index| !streams[*index].active) else {
        return Err(rt::Error::CapacityExceeded);
    };
    let pair = rt::channel_create()?;
    let capture = CaptureStreamState {
        format,
        rate_hz,
        channels,
        start_tick: rt::monotonic_now().unwrap_or(0),
        frames_produced: 0,
        checksum: CHECKSUM_SEED,
    };
    streams[slot_index] = StreamSlot {
        active: true,
        control_handle: pair.first,
        session_id,
        endpoint_index: 0,
        frequency_hz: 0,
        until_tick: 0,
        state: AudioStreamState::Idle,
        pcm_configured: false,
        direction: AudioStreamDirection::Capture,
        capture: Some(capture),
    };
    pcm[slot_index].reset();
    Ok((slot_index, pair.second, capture))
}

/// Per-slot request handling for capture streams: reads (blocking or
/// not), status, and close. Any playback-only tag is answered
/// Unsupported with the mirrored reply tag.
fn handle_capture_stream_request(
    slot_index: usize,
    request: &RawMessage,
    log_handle: rt::Handle,
    streams: &mut [StreamSlot; MAX_AUDIO_STREAMS],
    pcm: &mut [PcmStreamState; MAX_AUDIO_STREAMS],
) -> rt::Result<()> {
    match request.tag {
        x if x == AudioTag::CaptureReadRequest as u32 => {
            if request.handle_count < 1 || request.word_count < 2 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(AudioTag::CaptureReadReply as u32);
            reply.word_count = CAPTURE_REPLY_HEADER_WORDS as u32;
            // Copy the config out; counters are written back below.
            let mut capture = streams[slot_index].capture.unwrap_or(CaptureStreamState {
                format: AudioSampleFormat::S16Le,
                rate_hz: SINK_RATE_HZ,
                channels: 2,
                start_tick: 0,
                frames_produced: 0,
                checksum: CHECKSUM_SEED,
            });
            let requested = (request.words[0] as usize).min(CAPTURE_MAX_READ_FRAMES);
            let blocking =
                request.words[1] & audio_stream_read_flag::BLOCKING != 0;
            // One reply carries at most the sample payload that fits in
            // the IPC word budget after the header words.
            let payload_words = IPC_MAX_WORDS - CAPTURE_REPLY_HEADER_WORDS;
            let capacity_frames = payload_words
                .saturating_mul(pcm_samples_per_word(capture.format))
                / capture.channels.max(1) as usize;
            let take_max = requested.min(capacity_frames);
            let mut now = rt::monotonic_now().unwrap_or(0);
            let deadline = now.saturating_add(CAPTURE_BLOCK_TICKS);
            // Hard iteration cap keeps a stalled clock from wedging the
            // service loop even with the blocking flag set.
            let mut spins = 0u64;
            let take = loop {
                let due = capture_frames_due(
                    capture.frames_produced,
                    capture.start_tick,
                    now,
                    capture.rate_hz,
                ) as usize;
                let take = due.min(take_max);
                if take > 0 {
                    break take;
                }
                spins += 1;
                if !blocking
                    || now >= deadline
                    || spins >= CAPTURE_BLOCK_TICKS * 16
                {
                    break 0;
                }
                let _ = rt::yield_current();
                now = rt::monotonic_now().unwrap_or(0);
            };
            if take > 0 {
                let first_tick =
                    capture_frame_tick(capture.start_tick, capture.frames_produced, capture.rate_hz);
                let packed =
                    capture_pack_silence(capture.format, capture.channels, take, &mut reply.words[CAPTURE_REPLY_HEADER_WORDS..]);
                capture.checksum = capture_checksum_silence(capture.checksum, take);
                capture.frames_produced += take as u64;
                streams[slot_index].capture = Some(capture);
                streams[slot_index].state = AudioStreamState::Active;
                reply.words[0] = AudioStatus::Ok as u32 as u64;
                reply.words[1] = take as u64;
                reply.words[2] = first_tick;
                reply.word_count = (CAPTURE_REPLY_HEADER_WORDS + packed) as u32;
            } else {
                // Real-time pacing has nothing due yet (or the read asked
                // for zero frames); Busy mirrors the nonblocking write path.
                reply.words[0] = AudioStatus::Busy as u32 as u64;
                reply.words[1] = 0;
                reply.words[2] = capture_frame_tick(
                    capture.start_tick,
                    capture.frames_produced,
                    capture.rate_hz,
                );
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == AudioTag::StreamStatusRequest as u32 => {
            if request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let stream = streams[slot_index];
            let capture = stream.capture.unwrap_or(CaptureStreamState {
                format: AudioSampleFormat::S16Le,
                rate_hz: SINK_RATE_HZ,
                channels: 2,
                start_tick: 0,
                frames_produced: 0,
                checksum: CHECKSUM_SEED,
            });
            let mut reply = RawMessage::empty(AudioTag::StreamStatusReply as u32);
            reply.word_count = 14;
            reply.words[0] = AudioStatus::Ok as u32 as u64;
            reply.words[1] = slot_index as u64;
            reply.words[2] = AudioStreamDirection::Capture as u32 as u64;
            reply.words[3] = stream.state as u32 as u64;
            reply.words[4] = stream.session_id as u64;
            reply.words[5] = stream.endpoint_index as u64;
            reply.words[6] = 0;
            reply.words[7] = 0;
            reply.words[8] = capture.rate_hz as u64;
            reply.words[9] = capture.channels as u64;
            reply.words[10] = capture.format as u32 as u64;
            reply.words[11] = 100;
            reply.words[12] = 0;
            reply.words[13] = capture.frames_produced;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == AudioTag::StreamCloseRequest as u32 => {
            if request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let produced = streams[slot_index]
                .capture
                .map(|capture| capture.frames_produced)
                .unwrap_or(0);
            close_stream_slot(streams, pcm, slot_index);
            let mut reply = RawMessage::empty(AudioTag::StreamCloseReply as u32);
            reply.word_count = 2;
            reply.words[0] = AudioStatus::Closed as u32 as u64;
            reply.words[1] = produced;
            let _ = emit_log(
                log_handle,
                LogSeverity::Info,
                LogEvent::AudioStreamClosed,
                slot_index as u64,
                produced,
            );
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        _ => {
            if let Some(reply_tag) = mirrored_playback_reply_tag(request.tag) {
                if request.handle_count > 0 {
                    let reply_handle = request.handles[0];
                    let mut reply = RawMessage::empty(reply_tag);
                    reply.word_count = 1;
                    reply.words[0] = AudioStatus::Unsupported as u32 as u64;
                    let _ = rt::channel_send(reply_handle, &reply);
                    let _ = rt::handle_close(reply_handle);
                }
            }
        }
    }
    Ok(())
}

/// Reply tag matching a playback-only request tag, for honest
/// Unsupported answers on capture slots.
fn mirrored_playback_reply_tag(tag: u32) -> Option<u32> {
    match tag {
        x if x == AudioTag::StreamPlayToneRequest as u32 => {
            Some(AudioTag::StreamPlayToneReply as u32)
        }
        x if x == AudioTag::StreamConfigureRequest as u32 => {
            Some(AudioTag::StreamConfigureReply as u32)
        }
        x if x == AudioTag::StreamWriteRequest as u32 => Some(AudioTag::StreamWriteReply as u32),
        x if x == AudioTag::StreamDrainRequest as u32 => Some(AudioTag::StreamDrainReply as u32),
        x if x == AudioTag::StreamSetVolumeRequest as u32 => {
            Some(AudioTag::StreamSetVolumeReply as u32)
        }
        _ => None,
    }
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
