use serviceos_userspace_runtime as rt;

pub(crate) const BUFFER_WIDTH: u32 = 800;
pub(crate) const BUFFER_HEIGHT: u32 = 600;
pub(crate) const BUFFER_BYTES: usize = BUFFER_WIDTH as usize * BUFFER_HEIGHT as usize * 4;
pub(crate) const SURFACE_BUFFER_SLOTS: usize = 2;
pub(crate) const PIXEL_STRIDE: usize = BUFFER_WIDTH as usize;

pub(crate) const MAX_TRACKS: usize = 24;
pub(crate) const MAX_NAME: usize = 64;
pub(crate) const MAX_PATH: usize = 96;
/// Files larger than this are played truncated; the UI says so honestly.
pub(crate) const FILE_MAX_BYTES: usize = 192 * 1024;

pub(crate) const LIST_X: i32 = 14;
pub(crate) const LIST_Y: i32 = 62;
pub(crate) const ROW_HEIGHT: i32 = 22;

pub(crate) const BUTTON_Y: i32 = 520;
pub(crate) const BUTTON_W: i32 = 88;
pub(crate) const BUTTON_H: i32 = 30;
pub(crate) const PLAY_X: i32 = 14;
pub(crate) const STOP_X: i32 = 112;

pub(crate) const SESSION_ID: u32 = 0x4D45;
/// Packed sample words a single StreamWrite may carry (IPC minus header).
pub(crate) const PACKED_WORDS_MAX: usize = rt::IPC_MAX_WORDS - 2;

pub(crate) const KEY_ENTER: u32 = 28;
pub(crate) const KEY_MINUS: u32 = 12;
pub(crate) const KEY_EQUAL: u32 = 13;
pub(crate) const KEY_P: u32 = 25;
pub(crate) const KEY_S: u32 = 31;
pub(crate) const KEY_SPACE: u32 = 57;
pub(crate) const KEY_UP: u32 = 103;
pub(crate) const KEY_DOWN: u32 = 108;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PlayState {
    Idle,
    Playing,
}

#[derive(Clone, Copy)]
pub(crate) struct Track {
    pub(crate) path: [u8; MAX_PATH],
    pub(crate) path_len: usize,
}

impl Track {
    pub(crate) const fn empty() -> Self {
        Self {
            path: [0; MAX_PATH],
            path_len: 0,
        }
    }

    pub(crate) fn name_bytes(&self) -> &[u8] {
        let path = &self.path[..self.path_len];
        let start = path
            .iter()
            .rposition(|byte| *byte == b'/')
            .map(|index| index + 1)
            .unwrap_or(0);
        &path[start..]
    }
}

pub(crate) struct MediaState {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) focused: bool,
    pub(crate) scan_done: bool,
    pub(crate) scan_failed: bool,
    pub(crate) tracks: [Track; MAX_TRACKS],
    pub(crate) track_count: usize,
    pub(crate) selected: usize,
    pub(crate) play_state: PlayState,
    pub(crate) playing_track: usize,
    pub(crate) volume_percent: u8,
    pub(crate) muted: bool,
    /// Decoded source bytes of the loaded track.
    pub(crate) file_bytes: [u8; FILE_MAX_BYTES],
    pub(crate) file_len: usize,
    pub(crate) file_truncated: bool,
    pub(crate) stream_handle: rt::Handle,
    pub(crate) desktop_handle: rt::Handle,
    pub(crate) frame_cursor: usize,
    pub(crate) total_frames: usize,
    /// Active codec pipeline for the loaded track.
    pub(crate) decoder: Option<crate::codec::Decoder>,
    pub(crate) total_ms: u64,
    pub(crate) status_note: [u8; 96],
    pub(crate) status_note_len: usize,
}

impl MediaState {
    pub(crate) fn new(width: u32, height: u32, focused: bool) -> Self {
        Self {
            width,
            height,
            focused,
            scan_done: false,
            scan_failed: false,
            tracks: [Track::empty(); MAX_TRACKS],
            track_count: 0,
            selected: 0,
            play_state: PlayState::Idle,
            playing_track: usize::MAX,
            volume_percent: 80,
            muted: false,
            file_bytes: [0; FILE_MAX_BYTES],
            file_len: 0,
            file_truncated: false,
            stream_handle: rt::INVALID_HANDLE,
            desktop_handle: rt::INVALID_HANDLE,
            frame_cursor: 0,
            total_frames: 0,
            decoder: None,
            total_ms: 0,
            status_note: [0; 96],
            status_note_len: 0,
        }
    }

    pub(crate) fn set_note(&mut self, text: &[u8]) {
        let len = text.len().min(self.status_note.len());
        self.status_note[..len].copy_from_slice(&text[..len]);
        self.status_note_len = len;
    }

    pub(crate) fn note_bytes(&self) -> &[u8] {
        &self.status_note[..self.status_note_len]
    }
}
