use rt::{
    AudioEndpointBackend, AudioEndpointState, ControlTag, LogEvent, LogSeverity, RawMessage,
    ServiceId,
};
use serviceos_abi::{
    MIX_BATCH_FRAMES, PcmNullSink, PcmStreamState, audio_capability, run_pcm_mix_selftest,
    run_pcm_mix_selftest_emit,
};
use serviceos_userspace_runtime as rt;

use crate::{
    consts::MAX_AUDIO_STREAMS,
    protocol::{close_stream_slot, handle_public_request, handle_stream_request},
    types::StreamSlot,
    util::{emit_log, poll_lifecycle, update_stream_expiry},
};

/// One mixed batch of stereo s16 frames, the unit handed to the PCM sink.
const SINK_BATCH_BYTES: usize = MIX_BATCH_FRAMES * 4;

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

    // Boot selftest proves the mixed-PCM pipeline end to end without
    // touching the hardware endpoint. Evidence lands on the serial log.
    let mix_selftest = run_pcm_mix_selftest();
    let _ = rt::write_logf(
        "audio",
        format_args!(
            "selftest mix {} frames={} clip={} a={:#018x} b={:#018x} mixed={:#018x}",
            if mix_selftest.ok { "ok" } else { "FAILED" },
            mix_selftest.frames_mixed,
            mix_selftest.clipped_frames,
            mix_selftest.checksum_a,
            mix_selftest.checksum_b,
            mix_selftest.checksum_mixed,
        ),
    );

    // A PCM-capable endpoint (virtio-sound) becomes the real sink; the
    // null sink remains the honest fallback when only the PC speaker or
    // nothing is present.
    let mut pcm_sink_active = endpoint.backend == AudioEndpointBackend::VirtioSound
        && endpoint.capabilities & audio_capability::PCM != 0;
    let mut pcm_bytes_total = 0u64;
    if pcm_sink_active {
        let write_result = run_pcm_mix_selftest_emit(&mut |batch| {
            if let Ok(accepted) = rt::kernel_audio_endpoint_pcm_write(audio_handle, batch) {
                pcm_bytes_total = pcm_bytes_total.saturating_add(accepted as u64);
            }
        });
        let _ = rt::write_logf(
            "audio",
            format_args!(
                "selftest virtio {} frames={} bytes={} clip={}",
                if write_result.ok && pcm_bytes_total > 0 {
                    "ok"
                } else {
                    "FAILED"
                },
                write_result.frames_mixed,
                pcm_bytes_total,
                write_result.clipped_frames,
            ),
        );
        if !write_result.ok || pcm_bytes_total == 0 {
            // The device answered probe but refuses frames; degrade to the
            // null sink instead of pretending playback works.
            pcm_sink_active = false;
        }
    }
    let _ = rt::write_logf(
        "audio",
        format_args!(
            "sink={} bytes={}",
            if pcm_sink_active {
                "virtio-sound"
            } else {
                "null"
            },
            pcm_bytes_total,
        ),
    );

    let mut streams = [StreamSlot::empty(); MAX_AUDIO_STREAMS];
    let mut pcm = [const { PcmStreamState::new() }; MAX_AUDIO_STREAMS];
    let mut sink = PcmNullSink::new();

    loop {
        if poll_lifecycle(bootstrap).unwrap_or(false) {
            for slot in 0..streams.len() {
                if streams[slot].active {
                    close_stream_slot(&mut streams, &mut pcm, slot);
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
            && (endpoint.state == AudioEndpointState::Idle
                || streams[stopped_slot].frequency_hz == 0)
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
                if handle_public_request(
                    &request,
                    log_handle,
                    endpoint,
                    &mut streams,
                    &mut pcm,
                    &mut sink,
                )
                .is_err()
                {
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
                    if handle_stream_request(
                        slot,
                        &request,
                        audio_handle,
                        log_handle,
                        &mut streams,
                        &mut pcm,
                        &mut sink,
                    )
                    .is_err()
                    {
                        return 0xfa09;
                    }
                }
                Err(rt::Error::QueueEmpty) => {}
                Err(_) => {
                    close_stream_slot(&mut streams, &mut pcm, slot);
                }
            }
        }

        // Advance the mixer whenever any PCM stream has queued frames so
        // the sink counters/checksum progress even without new requests.
        // With a PCM endpoint active, every mixed batch is handed to the
        // virtio-sound device; a failed write permanently degrades to the
        // null sink (logged once) instead of failing the service.
        let pending: usize = pcm.iter().map(|stream| stream.ring.len()).sum();
        if pending > 0 {
            let mut batch = [0u8; SINK_BATCH_BYTES];
            let mixed = sink.mix_batch_into(&mut pcm, MIX_BATCH_FRAMES, &mut batch);
            had_work = true;
            if mixed > 0 && pcm_sink_active {
                match rt::kernel_audio_endpoint_pcm_write(audio_handle, &batch[..mixed * 4]) {
                    Ok(accepted) => pcm_bytes_total += accepted as u64,
                    Err(_) => {
                        pcm_sink_active = false;
                        let _ = emit_log(
                            log_handle,
                            LogSeverity::Error,
                            LogEvent::AudioEndpointReady,
                            0xDEAD,
                            0,
                        );
                    }
                }
            }
        }

        if !had_work && rt::yield_current().is_err() {
            return 0xfa0a;
        }
    }
}
