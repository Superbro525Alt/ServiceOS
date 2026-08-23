use rt::{
    AudioEndpointState, AudioEndpointStatusInfo, AudioStreamDirection, AudioStreamInfo,
    AudioStreamState, ServiceId,
};
use serviceos_userspace_runtime as rt;

use crate::util::{ShellOutput, write_output_linef};

const MAX_AUDIO_STREAMS: usize = 4;

pub(crate) fn cmd_audio<'a, I>(
    bootstrap: rt::Handle,
    output: ShellOutput,
    mut parts: I,
) -> rt::Result<()>
where
    I: Iterator<Item = &'a str>,
{
    match parts.next() {
        Some("endpoints") => cmd_audio_endpoints(bootstrap, output),
        Some("streams") => cmd_audio_streams(bootstrap, output),
        Some("tone") => match (
            parts.next().and_then(|value| value.parse::<u32>().ok()),
            parts
                .next()
                .and_then(|value| value.parse::<u32>().ok())
                .or(Some(150)),
        ) {
            (Some(frequency_hz), Some(duration_ms)) => {
                cmd_audio_tone(bootstrap, output, frequency_hz, duration_ms)
            }
            _ => write_output_linef(output, format_args!("usage: audio tone <hz> [ms]")),
        },
        _ => write_output_linef(
            output,
            format_args!("usage: audio <endpoints|streams|tone> ..."),
        ),
    }
}

fn cmd_audio_endpoints(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let audio_handle = rt::lookup_service(bootstrap, ServiceId::Audio)?;
    let count = rt::audio_service_endpoint_count(audio_handle)?;
    if count == 0 {
        let _ = rt::handle_close(audio_handle);
        return write_output_linef(output, format_args!("no audio endpoints"));
    }
    for index in 0..count {
        if let Some(endpoint) = rt::audio_service_endpoint_status(audio_handle, index)? {
            write_endpoint_line(output, endpoint)?;
        }
    }
    let _ = rt::handle_close(audio_handle);
    Ok(())
}

fn cmd_audio_streams(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let audio_handle = rt::lookup_service(bootstrap, ServiceId::Audio)?;
    let mut streams = [AudioStreamInfo {
        slot: 0,
        direction: AudioStreamDirection::Playback,
        state: AudioStreamState::Closed,
        session_id: 0,
        endpoint_index: 0,
        frequency_hz: 0,
        remaining_ticks: 0,
    }; MAX_AUDIO_STREAMS];
    let count = rt::audio_stream_list(audio_handle, &mut streams)?;
    let _ = rt::handle_close(audio_handle);
    if count == 0 {
        return write_output_linef(output, format_args!("no active audio streams"));
    }
    for stream in streams.iter().take(count).copied() {
        write_output_linef(
            output,
            format_args!(
                "stream{} state={} session={} endpoint={} freq={}Hz",
                stream.slot,
                stream_state_name(stream.state),
                stream.session_id,
                stream.endpoint_index,
                stream.frequency_hz,
            ),
        )?;
    }
    Ok(())
}

fn cmd_audio_tone(
    bootstrap: rt::Handle,
    output: ShellOutput,
    frequency_hz: u32,
    duration_ms: u32,
) -> rt::Result<()> {
    let audio_handle = rt::lookup_service(bootstrap, ServiceId::Audio)?;
    let stream_handle = rt::audio_stream_open(audio_handle, AudioStreamDirection::Playback, 0)?;
    let _ = rt::handle_close(audio_handle);
    rt::audio_stream_play_tone(stream_handle, frequency_hz, duration_ms)?;
    let deadline = rt::monotonic_now()?.saturating_add(duration_ms.div_ceil(10).max(1) as u64);
    while rt::monotonic_now()? < deadline {
        rt::yield_current()?;
    }
    let _ = rt::audio_stream_close(stream_handle);
    let _ = rt::handle_close(stream_handle);
    write_output_linef(
        output,
        format_args!("played {}Hz for {}ms", frequency_hz, duration_ms),
    )
}

fn write_endpoint_line(output: ShellOutput, endpoint: AudioEndpointStatusInfo) -> rt::Result<()> {
    write_output_linef(
        output,
        format_args!(
            "ep{} backend={} state={} caps={:#x} freq={}Hz range={}..{} plays={}",
            endpoint.index,
            endpoint_backend_name(endpoint.backend),
            endpoint_state_name(endpoint.state),
            endpoint.capabilities,
            endpoint.current_frequency_hz,
            endpoint.min_frequency_hz,
            endpoint.max_frequency_hz,
            endpoint.play_count,
        ),
    )
}

fn endpoint_backend_name(backend: rt::AudioEndpointBackend) -> &'static str {
    match backend {
        rt::AudioEndpointBackend::PcSpeaker => "pc-speaker",
        rt::AudioEndpointBackend::Unknown => "unknown",
    }
}

fn endpoint_state_name(state: AudioEndpointState) -> &'static str {
    match state {
        AudioEndpointState::Offline => "offline",
        AudioEndpointState::Idle => "idle",
        AudioEndpointState::Active => "active",
    }
}

fn stream_state_name(state: AudioStreamState) -> &'static str {
    match state {
        AudioStreamState::Idle => "idle",
        AudioStreamState::Active => "active",
        AudioStreamState::Closed => "closed",
        AudioStreamState::Failed => "failed",
    }
}
