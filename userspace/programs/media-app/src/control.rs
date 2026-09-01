use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;

use crate::audioclient;
use crate::library;
use crate::plan;
use crate::state::{
    BUTTON_H, BUTTON_W, BUTTON_Y, FILE_MAX_BYTES, KEY_DOWN, KEY_ENTER, KEY_EQUAL, KEY_LEFT,
    KEY_MINUS, KEY_P, KEY_RIGHT, KEY_S, KEY_SPACE, KEY_UP, LIST_Y, MAX_PATH, MediaState, PLAY_X,
    PlayState, ROW_HEIGHT, STOP_X,
};
use serviceos_userspace_runtime::AudioSampleFormat;

use crate::codec::{self, CodecError};

/// Nonblocking drain of the app control channel: focus, resize, pointer,
/// keys, and shell open-with handoffs. Returns whether state changed.
pub(crate) fn poll_control(
    control_handle: rt::Handle,
    state: &mut MediaState,
    storage_handle: rt::Handle,
    audio_handle: rt::Handle,
) -> rt::Result<bool> {
    let mut changed = false;
    loop {
        let mut message = rt::RawMessage::empty(0);
        match rt::channel_receive_nonblocking(control_handle, &mut message) {
            Ok(())
                if message.tag == rt::AppControlTag::FocusChanged as u32
                    && message.word_count > 0 =>
            {
                state.focused = message.words[0] != 0;
                changed = true;
            }
            Ok(())
                if message.tag == rt::AppControlTag::Resize as u32 && message.word_count >= 2 =>
            {
                state.width = message.words[0] as u32;
                state.height = message.words[1] as u32;
                changed = true;
            }
            Ok(())
                if message.tag == rt::AppControlTag::Pointer as u32 && message.word_count >= 5 =>
            {
                changed |= handle_pointer(
                    state,
                    storage_handle,
                    audio_handle,
                    message.words[1] as i64 as i32,
                    message.words[2] as i64 as i32,
                    message.words[0],
                );
            }
            Ok(()) if message.tag == rt::AppControlTag::Key as u32 && message.word_count >= 2 => {
                if matches!(
                    ui::decode_app_key_action(message.words[0]),
                    Some(rt::AppKeyAction::Down)
                ) {
                    changed |=
                        handle_key(state, storage_handle, audio_handle, message.words[1] as u32);
                }
            }
            Ok(())
                if message.tag == rt::AppControlTag::OpenPath as u32 && message.word_count >= 1 =>
            {
                let requested = message.words[0] as usize;
                let mut path = [0u8; MAX_PATH];
                if requested <= path.len()
                    && rt::unpack_bytes(
                        &message.words[1..message.word_count as usize],
                        requested,
                        &mut path,
                    )
                    .is_ok()
                {
                    changed |=
                        open_intent_path(state, storage_handle, audio_handle, &path[..requested]);
                }
            }
            Ok(()) => {}
            Err(rt::Error::QueueEmpty) => break,
            Err(error) => return Err(error),
        }
    }
    Ok(changed)
}

fn handle_pointer(
    state: &mut MediaState,
    storage_handle: rt::Handle,
    audio_handle: rt::Handle,
    x: i32,
    y: i32,
    action_word: u64,
) -> bool {
    if !matches!(
        ui::decode_app_pointer_action(action_word),
        Some(rt::AppPointerAction::Down)
    ) {
        return false;
    }
    if in_button(x, y, PLAY_X) {
        return play_selected(state, storage_handle, audio_handle);
    }
    if in_button(x, y, STOP_X) {
        return stop_playback(state, "MEDIA stopped");
    }
    let row_height = ROW_HEIGHT;
    let relative = y - LIST_Y + 2;
    if relative >= 0 && (relative / row_height) < crate::state::MAX_TRACKS as i32 {
        let index = (relative / row_height) as usize;
        if index < state.track_count {
            if index == state.selected {
                play_selected(state, storage_handle, audio_handle)
            } else {
                state.selected = index;
                true
            }
        } else {
            false
        }
    } else {
        false
    }
}

fn in_button(x: i32, y: i32, button_x: i32) -> bool {
    x >= button_x && x < button_x + BUTTON_W && y >= BUTTON_Y && y < BUTTON_Y + BUTTON_H
}

fn handle_key(
    state: &mut MediaState,
    storage_handle: rt::Handle,
    audio_handle: rt::Handle,
    key: u32,
) -> bool {
    match key {
        KEY_UP if state.track_count > 0 => {
            state.selected = state.selected.saturating_sub(1);
            true
        }
        KEY_DOWN if state.track_count > 0 => {
            state.selected = (state.selected + 1).min(state.track_count - 1);
            true
        }
        KEY_ENTER | KEY_P => play_selected(state, storage_handle, audio_handle),
        KEY_SPACE | KEY_S => stop_playback(state, "MEDIA stopped"),
        KEY_MINUS => apply_volume(state, -10),
        KEY_EQUAL => apply_volume(state, 10),
        KEY_LEFT => seek_playback(state, -10, audio_handle),
        KEY_RIGHT => seek_playback(state, 10, audio_handle),
        _ => false,
    }
}

/// Why one track could not be started, mapped to honest UI notes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StartError {
    Open,
    Read,
    Unsupported,
    Codec(CodecError),
    AudioUnavailable,
    ConfigRejected,
    WriteFailed,
}

fn start_error_note(error: StartError) -> &'static [u8] {
    match error {
        StartError::Open => b"OPEN FAILED",
        StartError::Read => b"READ FAILED",
        StartError::Unsupported => b"UNSUPPORTED FORMAT",
        StartError::Codec(err) => open_error_note(err),
        StartError::AudioUnavailable => b"AUDIO UNAVAILABLE",
        StartError::ConfigRejected => b"CONFIG REJECTED",
        StartError::WriteFailed => b"WRITE FAILED",
    }
}

/// Loads track `index` from storage and starts streaming it: sniff ->
/// decoder -> stream open/configure -> carried volume/mute -> first
/// chunk write so audio starts inside this control flow. Commits state
/// only on success; on failure any opened stream is closed and the
/// caller decides the note. `index` must be < track_count.
fn start_track(
    state: &mut MediaState,
    storage_handle: rt::Handle,
    audio_handle: rt::Handle,
    index: usize,
) -> Result<(), StartError> {
    let Some(track) = state.tracks.get(index) else {
        return Err(StartError::Open);
    };
    let path = match core::str::from_utf8(&track.path[..track.path_len]) {
        Ok(path) => path,
        Err(_) => return Err(StartError::Open),
    };
    let (blob, size) = match rt::storage_open(storage_handle, path) {
        Ok(opened) => opened,
        Err(_) => return Err(StartError::Open),
    };
    let read_len = size.min(FILE_MAX_BYTES);
    let file_len = match rt::storage_read_all(blob, &mut state.file_bytes[..read_len], read_len) {
        Ok(read) => read,
        Err(_) => {
            let _ = rt::handle_close(blob);
            return Err(StartError::Read);
        }
    };
    let _ = rt::handle_close(blob);

    let mut decoder =
        codec::Decoder::open(&state.file_bytes[..file_len]).map_err(StartError::Codec)?;
    let format = pipeline_format();

    let stream = match audioclient::stream_open(audio_handle) {
        Ok(stream) => stream,
        Err(_) => return Err(StartError::AudioUnavailable),
    };
    if audioclient::stream_configure(stream, format, decoder.sample_rate, decoder.channels).is_err()
    {
        let _ = audioclient::stream_close(stream);
        return Err(StartError::ConfigRejected);
    }
    // Volume and mute carry across track transitions by construction.
    let _ = audioclient::stream_set_volume(stream, state.volume_percent, state.muted);

    // First chunk lands before returning, so the transition begins
    // audible without waiting for the next pump tick.
    let channels = decoder.channels as usize;
    let chunk_frames = plan::frames_per_chunk(decoder.channels, format);
    let want_frames =
        plan::chunk_sample_count(decoder.total_frames(), chunk_frames, decoder.channels)
            / channels.max(1);
    let mut sample_buffer = [0f32; plan::SERVICE_SAMPLE_BUFFER];
    let decoded_frames = decoder.decode_next(
        &state.file_bytes[..file_len],
        want_frames,
        &mut sample_buffer,
    );
    if decoded_frames > 0 {
        let used = decoded_frames * channels;
        let mut words = [0u64; crate::state::PACKED_WORDS_MAX];
        let word_count = audioclient::pack_samples(format, &sample_buffer[..used], &mut words);
        if audioclient::stream_write(stream, decoded_frames, &words[..word_count]).is_err() {
            let _ = audioclient::stream_close(stream);
            return Err(StartError::WriteFailed);
        }
    }

    state.stream_handle = stream;
    state.file_len = file_len;
    state.file_truncated = size > FILE_MAX_BYTES;
    state.playing_track = index;
    state.selected = index;
    state.frame_cursor = decoded_frames;
    state.decoder = Some(decoder);
    state.total_frames = state
        .decoder
        .map(|dec| dec.total_frames())
        .unwrap_or_default();
    state.total_ms = state.decoder.map(|dec| dec.duration_ms()).unwrap_or(0);
    state.play_state = PlayState::Playing;
    Ok(())
}

/// Loads the selected track's bytes and starts streaming it.
pub(crate) fn play_selected(
    state: &mut MediaState,
    storage_handle: rt::Handle,
    audio_handle: rt::Handle,
) -> bool {
    stop_playback(state, "MEDIA stopped");
    if state.track_count == 0 {
        state.set_note(b"NO TRACKS");
        return true;
    }
    let index = state.selected.min(state.track_count.saturating_sub(1));
    match start_track(state, storage_handle, audio_handle, index) {
        Ok(()) => state.set_note(b"PLAYING"),
        Err(error) => state.set_note(start_error_note(error)),
    }
    true
}

/// Honest status-note text for every registry rejection reason.
fn open_error_note(err: CodecError) -> &'static [u8] {
    match err {
        CodecError::NotWav => b"UNSUPPORTED FORMAT",
        CodecError::UnsupportedEncoding | CodecError::BadHeader => b"CODEC UNSUPPORTED",
    }
}

/// Jumps the active stream by `delta_secs` (negative = back). The stream
/// contract has no position primitive, so seeking re-opens the stream
/// from the new decode offset: no stale pre-seek audio lingers in the
/// service ring. Per-sample encodings (PCM, G.711) land exactly;
/// block-compressed IMA lands on the containing block boundary and
/// drops intra-block frames when the header's wSamplesPerBlock backs
/// the frame→block math (block-granular landing otherwise).
pub(crate) fn seek_playback(
    state: &mut MediaState,
    delta_secs: i32,
    audio_handle: rt::Handle,
) -> bool {
    if state.play_state != PlayState::Playing || state.playing_track == usize::MAX {
        state.set_note(b"NOT PLAYING");
        return true;
    }
    let Some(mut decoder) = state.decoder else {
        return false;
    };
    let target = codec::seek_target_frame(
        state.frame_cursor,
        delta_secs,
        decoder.sample_rate,
        state.total_frames,
    );
    if !decoder.seek_frames(target) {
        state.set_note(b"SEEK NOT SUPPORTED");
        return true;
    }
    if state.stream_handle != rt::INVALID_HANDLE {
        let _ = audioclient::stream_close(state.stream_handle);
        state.stream_handle = rt::INVALID_HANDLE;
    }
    let stream = match audioclient::stream_open(audio_handle) {
        Ok(stream) => stream,
        Err(_) => {
            state.play_state = PlayState::Idle;
            state.set_note(b"AUDIO UNAVAILABLE");
            return true;
        }
    };
    if audioclient::stream_configure(
        stream,
        pipeline_format(),
        decoder.sample_rate,
        decoder.channels,
    )
    .is_err()
    {
        let _ = audioclient::stream_close(stream);
        state.play_state = PlayState::Idle;
        state.set_note(b"CONFIG REJECTED");
        return true;
    }
    let _ = audioclient::stream_set_volume(stream, state.volume_percent, state.muted);
    state.stream_handle = stream;
    state.frame_cursor = target;
    state.decoder = Some(decoder);
    let note: &[u8] = if delta_secs < 0 {
        b"SEEK -10S"
    } else {
        b"SEEK +10S"
    };
    state.set_note(note);
    true
}

/// Wire format fed into audio-service: decoders normalize to f32.
const fn pipeline_format() -> AudioSampleFormat {
    AudioSampleFormat::F32Le
}

/// Closes any active stream and posts `message` to notification history.
pub(crate) fn stop_playback(state: &mut MediaState, message: &str) -> bool {
    let was_active = state.play_state != PlayState::Idle || state.playing_track != usize::MAX;
    if state.stream_handle != rt::INVALID_HANDLE {
        let _ = audioclient::stream_close(state.stream_handle);
        state.stream_handle = rt::INVALID_HANDLE;
    }
    state.play_state = PlayState::Idle;
    state.playing_track = usize::MAX;
    state.frame_cursor = 0;
    state.total_frames = 0;
    state.decoder = None;
    if was_active {
        post_notification(state, message);
        true
    } else {
        false
    }
}

/// Bounded skip budget: at most this many successor loads are attempted
/// per finish before the app lands idle with an honest notice.
const TRANSITION_ATTEMPTS: usize = 3;

/// Next sequential index after `from`, or None at the end of the list
/// (the app stops at the end; no loop flag exists).
fn next_transition_index(from: usize, track_count: usize) -> Option<usize> {
    if from != usize::MAX && from + 1 < track_count {
        Some(from + 1)
    } else {
        None
    }
}

/// Boot-tick source, stubbed in host tests where syscalls do not exist.
fn now_ticks() -> u64 {
    #[cfg(test)]
    {
        0
    }
    #[cfg(not(test))]
    {
        rt::monotonic_now().unwrap_or(0)
    }
}

/// Sequential gapless core: starting at `finished_index + 1`, attempt up
/// to `attempts_budget` track starts, skipping forward past failures.
/// Returns true when a successor is Playing (state committed by the
/// starter). Host tests inject a fake starter; the live path passes
/// `start_track`.
fn transition_with(
    state: &mut MediaState,
    finished_index: usize,
    attempts_budget: usize,
    mut start: impl FnMut(&mut MediaState, usize) -> Result<(), StartError>,
) -> bool {
    let mut attempts = 0;
    let mut candidate = next_transition_index(finished_index, state.track_count);
    while let Some(index) = candidate {
        if attempts >= attempts_budget {
            break;
        }
        attempts += 1;
        if start(state, index).is_ok() {
            return true;
        }
        candidate = next_transition_index(index, state.track_count);
    }
    false
}

/// Fires when all frames drained: closes the stream, then pipelines the
/// next track in the same control flow — successor load, stream open,
/// configure, volume carry, and first write happen immediately after
/// the drained reply, so no operator-visible stop/idle round-trip sits
/// between tracks. Load or stream failures skip forward (bounded) or
/// land idle with an honest notice; the list is never wedged.
fn finish_playback(
    state: &mut MediaState,
    storage_handle: rt::Handle,
    audio_handle: rt::Handle,
) -> bool {
    let mut name_buffer = [0u8; 48];
    let name_len = finish_message_name(state, &mut name_buffer);
    let finished_index = state.playing_track;
    let started_tick = now_ticks();

    if state.stream_handle != rt::INVALID_HANDLE {
        let _ = audioclient::stream_drain(state.stream_handle);
        let _ = audioclient::stream_close(state.stream_handle);
        state.stream_handle = rt::INVALID_HANDLE;
    }
    state.decoder = None;
    state.frame_cursor = 0;
    state.total_frames = 0;

    // Gapless attempt: advance sequentially through the sorted list.
    // A write failure mid-track also lands here and abandons the
    // remainder honestly (the finished-track name was captured above).
    if transition_with(
        state,
        finished_index,
        TRANSITION_ATTEMPTS,
        |state, index| start_track(state, storage_handle, audio_handle, index),
    ) {
        let body_len = transition_message_body(state, &mut name_buffer).len();
        let total_tick = now_ticks();
        let _ = rt::write_logf(
            "media",
            format_args!(
                "gapless transition to track {} ticks={} from track {}",
                state.playing_track,
                total_tick.saturating_sub(started_tick),
                finished_index,
            ),
        );
        let note = assemble_media_note(b"MEDIA next: ", &name_buffer[..body_len]);
        let total_len = b"MEDIA next: ".len() + body_len;
        state.set_note(&note[..total_len]);
        post_notification(
            state,
            core::str::from_utf8(&note[..total_len]).unwrap_or("MEDIA next"),
        );
        return true;
    }

    state.play_state = PlayState::Idle;
    state.playing_track = usize::MAX;
    let note = assemble_media_note(b"MEDIA finished: ", &name_buffer[..name_len]);
    let total_len = b"MEDIA finished: ".len() + name_len;
    let message = core::str::from_utf8(&note[..total_len]).unwrap_or("MEDIA finished");
    state.set_note(&note[..total_len]);
    post_notification(state, message);
    true
}

/// Name bytes of the track now playing (after a successful transition).
fn transition_message_body<'a>(state: &MediaState, out: &'a mut [u8]) -> &'a [u8] {
    let track = state
        .tracks
        .get(state.playing_track)
        .cloned()
        .unwrap_or(crate::state::Track::empty());
    let name = track.name_bytes();
    let len = name.len().min(out.len());
    out[..len].copy_from_slice(&name[..len]);
    &out[..len]
}

/// Prefix + body assembly shared by the finished/next notifications.
fn assemble_media_note(prefix: &[u8], body: &[u8]) -> [u8; 64] {
    let mut note = [0u8; 64];
    let body_len = body.len().min(note.len() - prefix.len());
    note[..prefix.len()].copy_from_slice(prefix);
    note[prefix.len()..prefix.len() + body_len].copy_from_slice(&body[..body_len]);
    note
}

fn finish_message_name(state: &MediaState, out: &mut [u8]) -> usize {
    let track = state
        .tracks
        .get(state.playing_track.min(state.track_count))
        .cloned()
        .unwrap_or(crate::state::Track::empty());
    let name = track.name_bytes();
    let len = name.len().min(out.len());
    out[..len].copy_from_slice(&name[..len]);
    len
}

/// Posts plain-text into the shell notify channel; non-intent text becomes
/// a notification-history event with this app as source.
fn post_notification(state: &mut MediaState, text: &str) {
    // The desktop handle is stashed in place of a second copy: reuse the
    // audio handle slot is wrong, so main() passes it via set_desktop().
    let desktop = state.desktop_handle;
    if desktop == rt::INVALID_HANDLE {
        return;
    }
    let _ = rt::desktop_notify(desktop, text);
}

/// Submits up to `CHUNKS_PER_TICK` write chunks per loop tick so the UI
/// stays responsive. Returns whether anything changed.
pub(crate) fn pump_playback(
    state: &mut MediaState,
    storage_handle: rt::Handle,
    audio_handle: rt::Handle,
) -> bool {
    const CHUNKS_PER_TICK: usize = 4;
    if state.play_state != PlayState::Playing || state.stream_handle == rt::INVALID_HANDLE {
        return false;
    }
    let Some(mut decoder) = state.decoder else {
        return finish_playback(state, storage_handle, audio_handle);
    };
    let channels = decoder.channels as usize;
    let format = pipeline_format();
    let chunk_frames = plan::frames_per_chunk(decoder.channels, format);
    let data_end = state.file_len;

    for _ in 0..CHUNKS_PER_TICK {
        if state.frame_cursor >= state.total_frames {
            break;
        }
        let remaining = state.total_frames - state.frame_cursor;
        let want_frames =
            plan::chunk_sample_count(remaining, chunk_frames, decoder.channels) / channels.max(1);
        let mut sample_buffer = [0f32; plan::SERVICE_SAMPLE_BUFFER];
        let decoded_frames = decoder.decode_next(
            &state.file_bytes[..data_end],
            want_frames,
            &mut sample_buffer,
        );
        if decoded_frames == 0 {
            break;
        }
        let used = decoded_frames * channels;
        let mut words = [0u64; crate::state::PACKED_WORDS_MAX];
        let word_count = audioclient::pack_samples(format, &sample_buffer[..used], &mut words);
        match audioclient::stream_write(state.stream_handle, decoded_frames, &words[..word_count]) {
            Ok(_) => {
                state.frame_cursor += decoded_frames;
                state.decoder = Some(decoder);
            }
            Err(rt::Error::Busy) => break,
            Err(_) => {
                state.decoder = Some(decoder);
                state.set_note(b"WRITE FAILED");
                return finish_playback(state, storage_handle, audio_handle);
            }
        }
    }
    if !state.decoder.is_some() {
        state.decoder = Some(decoder);
    }
    if state.frame_cursor >= state.total_frames || decoder.total_frames() == 0 {
        return finish_playback(state, storage_handle, audio_handle);
    }
    false
}

pub(crate) fn apply_volume(state: &mut MediaState, delta: i32) -> bool {
    let next = (i32::from(state.volume_percent) + delta).clamp(0, 100) as u8;
    if next != state.volume_percent || (delta == 0 && !state.muted) {
        state.volume_percent = next;
        state.muted = false;
        if state.stream_handle != rt::INVALID_HANDLE {
            let _ = audioclient::stream_set_volume(state.stream_handle, next, false);
        }
        return true;
    }
    false
}

pub(crate) fn toggle_mute(state: &mut MediaState) -> bool {
    state.muted = !state.muted;
    if state.stream_handle != rt::INVALID_HANDLE {
        let _ =
            audioclient::stream_set_volume(state.stream_handle, state.volume_percent, state.muted);
    }
    true
}

/// Opens a file pushed by the shell (open-with handoff): registers it at
/// the top of the list, selects it, and starts playback.
pub(crate) fn open_intent_path(
    state: &mut MediaState,
    storage_handle: rt::Handle,
    audio_handle: rt::Handle,
    path: &[u8],
) -> bool {
    if path.len() > MAX_PATH {
        return false;
    }
    library::push_track_at_front(state, path);
    state.selected = 0;
    play_selected(state, storage_handle, audio_handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::CodecError;
    use crate::state::MAX_PATH;

    #[test]
    fn codec_errors_map_to_honest_notices() {
        assert_eq!(
            open_error_note(CodecError::NotWav),
            b"UNSUPPORTED FORMAT".as_slice()
        );
        assert_eq!(
            open_error_note(CodecError::UnsupportedEncoding),
            b"CODEC UNSUPPORTED".as_slice()
        );
        assert_eq!(
            open_error_note(CodecError::BadHeader),
            b"CODEC UNSUPPORTED".as_slice()
        );
    }

    #[test]
    fn pipeline_streams_normalized_f32() {
        // Every decoder feeds the stream as f32 stereo-capable frames; the
        // service config always uses F32Le regardless of source encoding.
        assert_eq!(pipeline_format(), AudioSampleFormat::F32Le);
    }

    #[test]
    fn seek_stays_honest_when_idle() {
        // Idle seeks reject before any IPC touch, so INVALID handles are
        // safe here by construction.
        let mut state = MediaState::new(800, 600, true);
        assert!(seek_playback(&mut state, 10, rt::INVALID_HANDLE));
        assert_eq!(state.note_bytes(), b"NOT PLAYING".as_slice());
        assert_eq!(state.play_state, PlayState::Idle);
        assert!(state.decoder.is_none());
    }

    fn track_at(state: &mut MediaState, index: usize, name: &[u8]) {
        let mut path = [0u8; MAX_PATH];
        path[..name.len()].copy_from_slice(name);
        state.tracks[index] = crate::state::Track {
            path,
            path_len: name.len(),
        };
        state.track_count = state.track_count.max(index + 1);
    }

    /// Mimics start_track's commit so fake-starter transitions exercise
    /// the same state surface without touching IPC.
    fn fake_start(
        fail_until: Option<usize>,
    ) -> impl FnMut(&mut MediaState, usize) -> Result<(), StartError> {
        move |state, index| {
            if let Some(limit) = fail_until
                && index <= limit
            {
                return Err(StartError::Open);
            }
            state.playing_track = index;
            state.selected = index;
            state.play_state = PlayState::Playing;
            state.frame_cursor = 0;
            state.total_frames = 1000;
            Ok(())
        }
    }

    #[test]
    fn transition_index_is_sequential_and_stops_at_list_end() {
        assert_eq!(next_transition_index(0, 3), Some(1));
        assert_eq!(next_transition_index(1, 3), Some(2));
        // Last track: the app stops (no loop flag exists).
        assert_eq!(next_transition_index(2, 3), None);
        assert_eq!(next_transition_index(usize::MAX, 0), None);
    }

    #[test]
    fn transition_skips_broken_successors_and_plays_next_good_track() {
        let mut state = MediaState::new(800, 600, true);
        track_at(&mut state, 0, b"one.wav");
        track_at(&mut state, 1, b"broken.wav");
        track_at(&mut state, 2, b"two.wav");
        state.playing_track = 0;
        let started = transition_with(&mut state, 0, TRANSITION_ATTEMPTS, fake_start(Some(1)));
        assert!(started);
        assert_eq!(state.playing_track, 2);
        assert_eq!(state.selected, 2);
        assert_eq!(state.play_state, PlayState::Playing);
        assert_eq!(state.frame_cursor, 0);
    }

    #[test]
    fn transition_attempts_are_bounded_then_lands_idle() {
        let mut state = MediaState::new(800, 600, true);
        track_at(&mut state, 0, b"one.wav");
        track_at(&mut state, 1, b"a.wav");
        track_at(&mut state, 2, b"b.wav");
        track_at(&mut state, 3, b"c.wav");
        state.playing_track = 0;
        let mut attempts = 0;
        let started = transition_with(&mut state, 0, TRANSITION_ATTEMPTS, |state, index| {
            attempts += 1;
            let _ = state;
            let _ = index;
            Err(StartError::AudioUnavailable)
        });
        assert!(!started);
        assert_eq!(attempts, TRANSITION_ATTEMPTS);
    }

    #[test]
    fn transition_never_fires_past_the_last_track() {
        let mut state = MediaState::new(800, 600, true);
        track_at(&mut state, 0, b"only.wav");
        state.playing_track = 0;
        let mut calls = 0;
        let started = transition_with(&mut state, 0, TRANSITION_ATTEMPTS, |_state, _index| {
            calls += 1;
            Ok(())
        });
        assert!(!started);
        assert_eq!(calls, 0);
    }

    #[test]
    fn transition_carries_volume_and_mute_untouched() {
        let mut state = MediaState::new(800, 600, true);
        track_at(&mut state, 0, b"one.wav");
        track_at(&mut state, 1, b"two.wav");
        state.playing_track = 0;
        state.volume_percent = 37;
        state.muted = true;
        let started = transition_with(&mut state, 0, TRANSITION_ATTEMPTS, fake_start(None));
        assert!(started);
        assert_eq!(state.volume_percent, 37);
        assert!(state.muted);
    }

    #[test]
    fn finish_lands_idle_with_honest_notice_at_list_end() {
        let mut state = MediaState::new(800, 600, true);
        track_at(&mut state, 0, b"one.wav");
        state.play_state = PlayState::Playing;
        state.playing_track = 0;
        state.volume_percent = 55;
        // INVALID stream/storage/audio handles: the drain is skipped, no
        // successor exists, so no IPC happens and the test stays host-safe.
        assert!(finish_playback(
            &mut state,
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE
        ));
        assert_eq!(state.play_state, PlayState::Idle);
        assert_eq!(state.playing_track, usize::MAX);
        assert_eq!(state.note_bytes(), b"MEDIA finished: one.wav".as_slice());
        assert_eq!(state.volume_percent, 55);
        assert!(state.decoder.is_none());
        assert_eq!(state.stream_handle, rt::INVALID_HANDLE);
    }

    #[test]
    fn start_errors_map_to_honest_notes() {
        assert_eq!(
            start_error_note(StartError::Open),
            b"OPEN FAILED".as_slice()
        );
        assert_eq!(
            start_error_note(StartError::Read),
            b"READ FAILED".as_slice()
        );
        assert_eq!(
            start_error_note(StartError::Unsupported),
            b"UNSUPPORTED FORMAT".as_slice()
        );
        assert_eq!(
            start_error_note(StartError::Codec(CodecError::BadHeader)),
            b"CODEC UNSUPPORTED".as_slice()
        );
        assert_eq!(
            start_error_note(StartError::AudioUnavailable),
            b"AUDIO UNAVAILABLE".as_slice()
        );
        assert_eq!(
            start_error_note(StartError::ConfigRejected),
            b"CONFIG REJECTED".as_slice()
        );
        assert_eq!(
            start_error_note(StartError::WriteFailed),
            b"WRITE FAILED".as_slice()
        );
    }

    #[test]
    fn pump_is_a_noop_when_idle() {
        let mut state = MediaState::new(800, 600, true);
        assert!(!pump_playback(
            &mut state,
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE
        ));
    }

    #[test]
    fn media_notes_assemble_without_padding() {
        let note = assemble_media_note(b"MEDIA next: ", b"two.wav");
        assert_eq!(&note[..12 + 7], b"MEDIA next: two.wav".as_slice());
    }
}
