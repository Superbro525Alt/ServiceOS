use core::fmt::Write;

use rt::{AudioStatus, AudioStreamInfo, AudioStreamState, AudioTag, FixedLogBuffer, RawMessage};
use serviceos_userspace_runtime as rt;

pub(crate) const MEDIA_OVERLAY_WIDTH: u32 = 360;
pub(crate) const MEDIA_OVERLAY_HEIGHT: u32 = 188;
pub(crate) const MEDIA_VOLUME_STEP: i32 = 10;
pub(crate) const MASTER_VOLUME_DEFAULT: u8 = 100;
pub(crate) const MEDIA_STREAM_VIEWS_MAX: usize = 4;
const MEDIA_HEADER_LINES: usize = 3;
pub(crate) const MEDIA_LINE_COUNT: usize = MEDIA_HEADER_LINES + MEDIA_STREAM_VIEWS_MAX;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MediaEndpointView {
    pub(crate) index: u32,
    pub(crate) backend_word: u32,
    pub(crate) state_word: u32,
    pub(crate) capabilities: u32,
    pub(crate) nominal_rate_hz: u32,
    pub(crate) channels: u32,
    pub(crate) play_count: u64,
    pub(crate) frames_mixed: u64,
    pub(crate) checksum: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MediaStreamView {
    pub(crate) slot: u32,
    pub(crate) direction_word: u32,
    pub(crate) state_word: u32,
    pub(crate) session_id: u32,
    pub(crate) endpoint_index: u32,
    pub(crate) frequency_hz: u32,
}

impl MediaStreamView {
    fn from_info(info: &AudioStreamInfo) -> Self {
        Self {
            slot: info.slot,
            direction_word: info.direction as u32,
            state_word: info.state as u32,
            session_id: info.session_id,
            endpoint_index: info.endpoint_index,
            frequency_hz: info.frequency_hz,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MediaSnapshot {
    pub(crate) available: bool,
    pub(crate) endpoint: Option<MediaEndpointView>,
    pub(crate) listed_streams: usize,
    pub(crate) active_streams: usize,
    pub(crate) streams: [Option<MediaStreamView>; MEDIA_STREAM_VIEWS_MAX],
}

impl MediaSnapshot {
    const fn unavailable() -> Self {
        Self {
            available: false,
            endpoint: None,
            listed_streams: 0,
            active_streams: 0,
            streams: [None; MEDIA_STREAM_VIEWS_MAX],
        }
    }
}

pub(crate) fn clamp_volume(volume: u64) -> u8 {
    if volume > 100 { 100 } else { volume as u8 }
}

pub(crate) fn step_volume(current: u8, delta: i32) -> u8 {
    (current as i32 + delta).clamp(0, 100) as u8
}

pub(crate) fn endpoint_state_name(word: u32) -> &'static str {
    match word {
        0 => "OFFLINE",
        1 => "IDLE",
        2 => "ACTIVE",
        _ => "UNKNOWN",
    }
}

pub(crate) fn backend_name(word: u32) -> &'static str {
    match word {
        1 => "PC-SPEAKER",
        _ => "UNKNOWN",
    }
}

pub(crate) fn stream_state_name(word: u32) -> &'static str {
    match word {
        1 => "IDLE",
        2 => "ACTIVE",
        3 => "CLOSED",
        4 => "FAILED",
        _ => "UNKNOWN",
    }
}

pub(crate) fn stream_direction_name(word: u32) -> &'static str {
    match word {
        1 => "PLAYBACK",
        2 => "CAPTURE",
        _ => "UNKNOWN",
    }
}

fn empty_stream_info() -> AudioStreamInfo {
    AudioStreamInfo {
        slot: 0,
        direction: rt::AudioStreamDirection::Playback,
        state: AudioStreamState::Closed,
        session_id: 0,
        endpoint_index: 0,
        frequency_hz: 0,
        remaining_ticks: 0,
    }
}

pub(crate) fn sample_media(audio_handle: rt::Handle) -> MediaSnapshot {
    if audio_handle == rt::INVALID_HANDLE {
        return MediaSnapshot::unavailable();
    }
    let mut snapshot = MediaSnapshot::unavailable();
    snapshot.available = true;

    let mut request = RawMessage::empty(AudioTag::EndpointStatusRequest as u32);
    request.word_count = 1;
    request.words[0] = 0;
    if let Ok(reply) = rt::channel_call(audio_handle, &mut request) {
        if reply.tag == AudioTag::EndpointStatusReply as u32
            && reply.word_count >= 14
            && reply.words[0] == AudioStatus::Ok as u32 as u64
        {
            snapshot.endpoint = Some(MediaEndpointView {
                index: reply.words[1] as u32,
                backend_word: reply.words[2] as u32,
                state_word: reply.words[4] as u32,
                capabilities: reply.words[5] as u32,
                nominal_rate_hz: reply.words[6] as u32,
                channels: reply.words[7] as u32,
                play_count: reply.words[11],
                frames_mixed: reply.words[12],
                checksum: reply.words[13],
            });
        }
    }

    let mut streams = [empty_stream_info(); MEDIA_STREAM_VIEWS_MAX];
    match rt::audio_stream_list(audio_handle, &mut streams) {
        Ok(count) => {
            snapshot.listed_streams = count;
            snapshot.active_streams = streams[..count]
                .iter()
                .filter(|stream| stream.state == AudioStreamState::Active)
                .count();
            for (view, info) in snapshot.streams.iter_mut().zip(streams[..count].iter()) {
                *view = Some(MediaStreamView::from_info(info));
            }
        }
        Err(_) => {}
    }
    snapshot
}

pub(crate) fn request_master_volume(
    audio_handle: rt::Handle,
    volume: u8,
    muted: bool,
) -> rt::Result<(u8, bool)> {
    if audio_handle == rt::INVALID_HANDLE {
        return Err(rt::Error::NotFound);
    }
    let mut request = RawMessage::empty(AudioTag::EndpointVolumeSetRequest as u32);
    request.word_count = 2;
    request.words[0] = volume as u64;
    request.words[1] = muted as u64;
    let reply = rt::channel_call(audio_handle, &mut request)?;
    if reply.tag != AudioTag::EndpointVolumeSetReply as u32 || reply.word_count < 3 {
        return Err(rt::Error::InvalidArgument);
    }
    let status = reply.words[0];
    if status != AudioStatus::Ok as u32 as u64 {
        return Err(rt::Error::Unknown(status));
    }
    Ok((clamp_volume(reply.words[1]), reply.words[2] != 0))
}

pub(crate) fn write_media_lines(
    snapshot: &MediaSnapshot,
    master_volume: u8,
    master_muted: bool,
    lines: &mut [FixedLogBuffer<48>; MEDIA_LINE_COUNT],
) -> usize {
    if !snapshot.available {
        let _ = write!(&mut lines[0], "AUDIO SERVICE UNAVAILABLE");
        return 1;
    }

    match snapshot.endpoint {
        Some(endpoint) => {
            let _ = write!(
                &mut lines[0],
                "EP{} {} {} RATE={} CH={} CAPS={:#x}",
                endpoint.index,
                backend_name(endpoint.backend_word),
                endpoint_state_name(endpoint.state_word),
                endpoint.nominal_rate_hz,
                endpoint.channels,
                endpoint.capabilities,
            );
            let _ = write!(
                &mut lines[1],
                "MIXED FRAMES={} CKSM={:x}",
                endpoint.frames_mixed, endpoint.checksum
            );
        }
        None => {
            let _ = write!(&mut lines[0], "NO AUDIO ENDPOINT");
            let _ = write!(&mut lines[1], "MIXED FRAMES=? CKSM=?");
        }
    }

    let _ = write!(
        &mut lines[2],
        "MASTER VOL={} {} PCM ACTIVE={}/{}",
        master_volume,
        if master_muted { "MUTED" } else { "UNMUTED" },
        snapshot.active_streams,
        snapshot.listed_streams,
    );

    let mut count = MEDIA_HEADER_LINES;
    for view in snapshot.streams.into_iter().flatten() {
        let _ = write!(
            &mut lines[count],
            "S{} {} {} SES={} EP={} {}HZ",
            view.slot,
            stream_direction_name(view.direction_word),
            stream_state_name(view.state_word),
            view.session_id,
            view.endpoint_index,
            view.frequency_hz,
        );
        count += 1;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamped_volume_caps_at_100_and_keeps_lower_range() {
        assert_eq!(clamp_volume(0), 0);
        assert_eq!(clamp_volume(100), 100);
        assert_eq!(clamp_volume(101), 100);
        assert_eq!(clamp_volume(u64::MAX), 100);
    }

    #[test]
    fn volume_steps_clamp_both_ends() {
        assert_eq!(step_volume(95, MEDIA_VOLUME_STEP), 100);
        assert_eq!(step_volume(100, MEDIA_VOLUME_STEP), 100);
        assert_eq!(step_volume(5, -MEDIA_VOLUME_STEP), 0);
        assert_eq!(step_volume(0, -MEDIA_VOLUME_STEP), 0);
        assert_eq!(step_volume(50, MEDIA_VOLUME_STEP), 60);
        assert_eq!(step_volume(50, -MEDIA_VOLUME_STEP), 40);
    }

    #[test]
    fn state_mapping_covers_contract_words_and_unknowns() {
        assert_eq!(endpoint_state_name(0), "OFFLINE");
        assert_eq!(endpoint_state_name(1), "IDLE");
        assert_eq!(endpoint_state_name(2), "ACTIVE");
        assert_eq!(endpoint_state_name(3), "UNKNOWN");

        assert_eq!(backend_name(1), "PC-SPEAKER");
        assert_eq!(backend_name(0), "UNKNOWN");

        assert_eq!(stream_state_name(1), "IDLE");
        assert_eq!(stream_state_name(2), "ACTIVE");
        assert_eq!(stream_state_name(3), "CLOSED");
        assert_eq!(stream_state_name(4), "FAILED");
        assert_eq!(stream_state_name(5), "UNKNOWN");

        assert_eq!(stream_direction_name(1), "PLAYBACK");
        assert_eq!(stream_direction_name(2), "CAPTURE");
        assert_eq!(stream_direction_name(0), "UNKNOWN");
    }

    #[test]
    fn unavailable_audio_reports_single_honest_line() {
        let snapshot = MediaSnapshot::unavailable();
        let mut lines = core::array::from_fn(|_| FixedLogBuffer::<48>::new());
        let count = write_media_lines(&snapshot, MASTER_VOLUME_DEFAULT, false, &mut lines);
        assert_eq!(count, 1);
        assert_eq!(lines[0].as_str(), "AUDIO SERVICE UNAVAILABLE");
    }

    #[test]
    fn missing_endpoint_renders_honest_absent_lines() {
        let mut snapshot = MediaSnapshot::unavailable();
        snapshot.available = true;
        let mut lines = core::array::from_fn(|_| FixedLogBuffer::<48>::new());
        let count = write_media_lines(&snapshot, 70, true, &mut lines);
        assert_eq!(count, 3);
        assert_eq!(lines[0].as_str(), "NO AUDIO ENDPOINT");
        assert_eq!(lines[1].as_str(), "MIXED FRAMES=? CKSM=?");
        assert_eq!(lines[2].as_str(), "MASTER VOL=70 MUTED PCM ACTIVE=0/0");
    }

    #[test]
    fn full_snapshot_renders_endpoint_sink_and_stream_lines() {
        let mut snapshot = MediaSnapshot::unavailable();
        snapshot.available = true;
        snapshot.endpoint = Some(MediaEndpointView {
            index: 0,
            backend_word: 1,
            state_word: 2,
            capabilities: 0b1101,
            nominal_rate_hz: 48_000,
            channels: 2,
            play_count: 7,
            frames_mixed: 4_096,
            checksum: 0xdead_beef,
        });
        snapshot.listed_streams = 2;
        snapshot.active_streams = 1;
        snapshot.streams[0] = Some(MediaStreamView {
            slot: 1,
            direction_word: 1,
            state_word: 2,
            session_id: 3,
            endpoint_index: 0,
            frequency_hz: 440,
        });
        snapshot.streams[1] = Some(MediaStreamView {
            slot: 2,
            direction_word: 1,
            state_word: 1,
            session_id: 3,
            endpoint_index: 0,
            frequency_hz: 0,
        });

        let mut lines = core::array::from_fn(|_| FixedLogBuffer::<48>::new());
        let count = write_media_lines(&snapshot, 100, false, &mut lines);
        assert_eq!(count, 5);
        assert_eq!(
            lines[0].as_str(),
            "EP0 PC-SPEAKER ACTIVE RATE=48000 CH=2 CAPS=0xd"
        );
        assert_eq!(lines[1].as_str(), "MIXED FRAMES=4096 CKSM=deadbeef");
        assert_eq!(lines[2].as_str(), "MASTER VOL=100 UNMUTED PCM ACTIVE=1/2");
        assert_eq!(lines[3].as_str(), "S1 PLAYBACK ACTIVE SES=3 EP=0 440HZ");
        assert_eq!(lines[4].as_str(), "S2 PLAYBACK IDLE SES=3 EP=0 0HZ");
    }
}
