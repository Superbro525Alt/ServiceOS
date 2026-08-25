use core::str;
use serviceos_userspace_runtime as rt;

use crate::state::{MAX_NAME, MAX_PATH, MAX_TRACKS, MediaState, Track};

/// Extensions this app can honestly play: RIFF/WAVE PCM containers and
/// headerless raw PCM (played as S16LE stereo at the sink rate).
pub(crate) fn is_audio_extension(ext: &[u8]) -> bool {
    let mut lower = [0u8; 8];
    if ext.is_empty() || ext.len() > lower.len() {
        return false;
    }
    lower[..ext.len()].copy_from_slice(ext);
    let key = &mut lower[..ext.len()];
    key.make_ascii_lowercase();
    match *key {
        [b'w', b'a', b'v'] | [b'w', b'a', b'v', b'e'] | [b'p', b'c', b'm'] => true,
        _ => false,
    }
}

pub(crate) fn extension_of(path: &[u8]) -> &[u8] {
    let name_start = path
        .iter()
        .rposition(|byte| *byte == b'/')
        .map(|index| index + 1)
        .unwrap_or(0);
    let name = &path[name_start..];
    match name.iter().rposition(|byte| *byte == b'.') {
        Some(dot) if dot != 0 && dot != name.len() - 1 => &name[dot + 1..],
        _ => &[],
    }
}

fn push_track(state: &mut MediaState, path: &[u8]) {
    if state.track_count >= MAX_TRACKS || path.is_empty() || path.len() > MAX_PATH {
        return;
    }
    if state.tracks[..state.track_count]
        .iter()
        .any(|track| track.path_len == path.len() && track.path[..track.path_len] == path[..])
    {
        return;
    }
    let mut slot = Track::empty();
    slot.path[..path.len()].copy_from_slice(path);
    slot.path_len = path.len();
    state.tracks[state.track_count] = slot;
    state.track_count += 1;
}

fn scan_directory(
    storage_handle: rt::Handle,
    prefix: &str,
    depth: usize,
    budget: &mut usize,
    state: &mut MediaState,
) -> rt::Result<()> {
    if *budget == 0 || state.track_count >= MAX_TRACKS {
        return Ok(());
    }
    *budget -= 1;
    let directory = rt::storage_open_directory(storage_handle, prefix, false)?;
    let mut index = 0usize;
    let mut path_buffer = [0u8; MAX_PATH];
    loop {
        match rt::storage_directory_read(directory, index, &mut path_buffer) {
            Ok(Some((next_index, kind, path_len))) => {
                if next_index <= index {
                    break;
                }
                index = next_index;
                let entry_path = &path_buffer[..path_len.min(MAX_PATH)];
                match kind {
                    rt::StorageEntryKind::File => {
                        if is_audio_extension(extension_of(entry_path)) {
                            let mut full = [0u8; MAX_PATH];
                            let joined = join_path(prefix, entry_path, &mut full);
                            push_track(state, &full[..joined]);
                        }
                    }
                    rt::StorageEntryKind::Directory => {
                        if depth < 2 {
                            let mut child = [0u8; MAX_PATH];
                            let joined = join_path(prefix, entry_path, &mut child);
                            if let Ok(child_text) = str::from_utf8(&child[..joined]) {
                                scan_directory(
                                    storage_handle,
                                    child_text,
                                    depth + 1,
                                    budget,
                                    state,
                                )?;
                            }
                        }
                    }
                }
                if state.track_count >= MAX_TRACKS {
                    break;
                }
            }
            Ok(None) => break,
            Err(error) => {
                let _ = rt::handle_close(directory);
                return Err(error);
            }
        }
    }
    let _ = rt::handle_close(directory);
    Ok(())
}

fn join_path<'a>(prefix: &str, name: &[u8], out: &'a mut [u8]) -> usize {
    let mut cursor = 0usize;
    for byte in prefix.as_bytes() {
        if cursor >= out.len() {
            return cursor;
        }
        out[cursor] = *byte;
        cursor += 1;
    }
    if cursor > 0 && out[cursor - 1] != b'/' && cursor < out.len() {
        out[cursor] = b'/';
        cursor += 1;
    }
    let room = out.len().saturating_sub(cursor).min(name.len());
    out[cursor..cursor + room].copy_from_slice(&name[..room]);
    cursor + room
}

/// Walks the app's storage scope collecting audio files. Failures degrade
/// to a partial list with `scan_failed` set.
pub(crate) fn scan_library(storage_handle: rt::Handle, state: &mut MediaState) {
    let mut budget = 32usize;
    state.track_count = 0;
    match scan_directory(storage_handle, "", 0, &mut budget, state) {
        Ok(()) => state.scan_failed = false,
        Err(_) => state.scan_failed = true,
    }
    state.scan_done = true;
    sort_tracks(state);
}

/// Registers a shell-pushed path at the top of the list (deduplicated).
pub(crate) fn push_track_at_front(state: &mut MediaState, path: &[u8]) {
    if path.is_empty() || path.len() > MAX_PATH {
        return;
    }
    let existing = (0..state.track_count).find(|index| {
        let track = &state.tracks[*index];
        track.path_len == path.len() && track.path[..track.path_len] == path[..]
    });
    if let Some(index) = existing {
        let moved = state.tracks[index];
        for slot in (1..=index).rev() {
            state.tracks[slot] = state.tracks[slot - 1];
        }
        state.tracks[0] = moved;
        return;
    }
    let mut slot = Track::empty();
    slot.path[..path.len()].copy_from_slice(path);
    slot.path_len = path.len();
    if state.track_count < MAX_TRACKS {
        for index in (1..=state.track_count).rev() {
            state.tracks[index] = state.tracks[index - 1];
        }
        state.track_count += 1;
        state.tracks[0] = slot;
    } else {
        for index in (1..MAX_TRACKS).rev() {
            state.tracks[index] = state.tracks[index - 1];
        }
        state.tracks[0] = slot;
    }
}

fn sort_tracks(state: &mut MediaState) {
    for i in 1..state.track_count {
        let current = state.tracks[i];
        let mut j = i;
        while j > 0 && track_key(&state.tracks[j - 1]) > track_key(&current) {
            state.tracks[j] = state.tracks[j - 1];
            j -= 1;
        }
        state.tracks[j] = current;
    }
}

fn track_key(track: &Track) -> [u8; MAX_NAME] {
    let mut key = [0u8; MAX_NAME];
    let name = track.name_bytes();
    let len = name.len().min(MAX_NAME);
    key[..len].copy_from_slice(&name[..len]);
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_extensions_match_case_insensitively() {
        assert!(is_audio_extension(b"wav"));
        assert!(is_audio_extension(b"WAV"));
        assert!(is_audio_extension(b"Wave"));
        assert!(is_audio_extension(b"pcm"));
        assert!(!is_audio_extension(b"txt"));
        assert!(!is_audio_extension(b"wavx"));
        assert!(!is_audio_extension(b""));
        assert!(!is_audio_extension(&[b'a'; 9]));
    }

    #[test]
    fn extensions_come_after_the_final_dot_only() {
        assert_eq!(extension_of(b"songs/loop.wav"), b"wav");
        assert_eq!(extension_of(b"songs/v2.WAV"), b"WAV");
        assert_eq!(extension_of(b"a.b/c"), b"");
        assert_eq!(extension_of(b".hidden"), b"");
        assert_eq!(extension_of(b"dir.d/"), b"");
        assert_eq!(extension_of(b""), b"");
    }
}
