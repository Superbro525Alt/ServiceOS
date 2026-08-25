use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;

use crate::audioclient;
use crate::library;
use crate::plan;
use crate::state::{
    BUTTON_H, BUTTON_W, BUTTON_Y, FILE_MAX_BYTES, KEY_DOWN, KEY_ENTER, KEY_EQUAL, KEY_MINUS, KEY_P,
    KEY_S, KEY_SPACE, KEY_UP, LIST_Y, MAX_PATH, MediaState, PLAY_X, PlayState, ROW_HEIGHT, STOP_X,
};
use crate::wav;

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
        _ => false,
    }
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
    let Some(track) = state.tracks.get(state.selected.min(state.track_count - 1)) else {
        return false;
    };
    let path = match core::str::from_utf8(&track.path[..track.path_len]) {
        Ok(path) => path,
        Err(_) => return false,
    };
    let (blob, size) = match rt::storage_open(storage_handle, path) {
        Ok(opened) => opened,
        Err(_) => {
            state.set_note(b"OPEN FAILED");
            return true;
        }
    };
    let read_len = size.min(FILE_MAX_BYTES);
    state.file_len = match rt::storage_read_all(blob, &mut state.file_bytes[..read_len], read_len) {
        Ok(read) => read,
        Err(_) => {
            let _ = rt::handle_close(blob);
            state.set_note(b"READ FAILED");
            return true;
        }
    };
    let _ = rt::handle_close(blob);
    state.file_truncated = size > FILE_MAX_BYTES;

    let Some(info) = wav::parse_wav(&state.file_bytes[..state.file_len]) else {
        state.set_note(b"UNSUPPORTED FORMAT");
        return true;
    };
    let Some(format) = info.sample_format() else {
        state.set_note(b"CODEC UNSUPPORTED");
        return true;
    };

    let stream = match audioclient::stream_open(audio_handle) {
        Ok(stream) => stream,
        Err(_) => {
            state.set_note(b"AUDIO UNAVAILABLE");
            return true;
        }
    };
    if audioclient::stream_configure(stream, format, info.sample_rate, info.channels).is_err() {
        let _ = audioclient::stream_close(stream);
        state.set_note(b"CONFIG REJECTED");
        return true;
    }
    let _ = audioclient::stream_set_volume(stream, state.volume_percent, state.muted);

    state.stream_handle = stream;
    state.playing_track = state.selected.min(state.track_count.saturating_sub(1));
    state.frame_cursor = 0;
    state.total_frames = info.frame_count();
    state.total_ms = info.duration_ms();
    state.play_state = PlayState::Playing;
    state.set_note(b"PLAYING");
    true
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
    if was_active {
        post_notification(state, message);
        true
    } else {
        false
    }
}

/// Fires when all frames drained: closes the stream and notifies history.
fn finish_playback(state: &mut MediaState) -> bool {
    let mut name_buffer = [0u8; 48];
    let name_len = finish_message_name(state, &mut name_buffer);
    if state.stream_handle != rt::INVALID_HANDLE {
        let _ = audioclient::stream_drain(state.stream_handle);
        let _ = audioclient::stream_close(state.stream_handle);
        state.stream_handle = rt::INVALID_HANDLE;
    }
    state.play_state = PlayState::Idle;
    state.playing_track = usize::MAX;
    state.frame_cursor = 0;
    state.total_frames = 0;
    let mut note = [0u8; 64];
    let prefix = b"MEDIA finished: ";
    let body_len = name_len.min(note.len() - prefix.len());
    note[..prefix.len()].copy_from_slice(prefix);
    note[prefix.len()..prefix.len() + body_len].copy_from_slice(&name_buffer[..body_len]);
    let total_len = prefix.len() + body_len;
    let message = core::str::from_utf8(&note[..total_len]).unwrap_or("MEDIA finished");
    state.set_note(&note[..total_len]);
    post_notification(state, message);
    true
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
pub(crate) fn pump_playback(state: &mut MediaState) -> bool {
    const CHUNKS_PER_TICK: usize = 4;
    if state.play_state != PlayState::Playing || state.stream_handle == rt::INVALID_HANDLE {
        return false;
    }
    let Some(info) = wav::parse_wav(&state.file_bytes[..state.file_len]) else {
        return finish_playback(state);
    };
    let Some(format) = info.sample_format() else {
        return finish_playback(state);
    };
    let chunk_frames = plan::frames_per_chunk(info.channels, format);
    let data_start = info.data_offset;
    let frame_bytes = info.frame_bytes().max(1);

    for _ in 0..CHUNKS_PER_TICK {
        if state.frame_cursor >= state.total_frames {
            break;
        }
        let remaining = state.total_frames - state.frame_cursor;
        let samples = plan::chunk_sample_count(remaining, chunk_frames, info.channels);
        let byte_span = samples * (info.bits_per_sample as usize / 8);
        let start = data_start + state.frame_cursor * frame_bytes;
        let mut sample_buffer = [0f32; plan::SERVICE_SAMPLE_BUFFER];
        let decoded = wav::decode_samples(
            &state.file_bytes[..state.file_len],
            start,
            samples,
            format,
            &mut sample_buffer,
        );
        let mut words = [0u64; crate::state::PACKED_WORDS_MAX];
        let word_count = audioclient::pack_samples(format, &sample_buffer[..decoded], &mut words);
        match audioclient::stream_write(
            state.stream_handle,
            decoded / info.channels as usize,
            &words[..word_count],
        ) {
            Ok(_) => {
                state.frame_cursor += decoded / info.channels as usize;
            }
            Err(rt::Error::Busy) => break,
            Err(_) => {
                state.set_note(b"WRITE FAILED");
                return finish_playback(state);
            }
        }
        if byte_span == 0 {
            break;
        }
    }
    if state.frame_cursor >= state.total_frames {
        return finish_playback(state);
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
