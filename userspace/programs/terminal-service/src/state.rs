use serviceos_userspace_runtime as rt;

pub(crate) const MAX_SESSIONS: usize = 4;
pub(crate) const MAX_LINE_BYTES: usize = 128;
pub(crate) const MAX_HISTORY: usize = 16;
pub(crate) const MAX_INLINE_BYTES: usize = (rt::IPC_MAX_WORDS - 1) * 8;
pub(crate) const DEFAULT_COLS: u32 = 80;
pub(crate) const DEFAULT_ROWS: u32 = 25;
pub(crate) const MAX_PUBLIC_REQUESTS_PER_TURN: usize = 8;
pub(crate) const MAX_SESSION_MESSAGES_PER_TURN: usize = 16;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum EscapeState {
    None,
    Esc,
    Csi,
}

#[derive(Clone, Copy)]
pub(crate) struct Session {
    pub(crate) endpoint: rt::Handle,
    pub(crate) id: u32,
    pub(crate) columns: u32,
    pub(crate) rows: u32,
    pub(crate) width_pixels: u32,
    pub(crate) height_pixels: u32,
    pub(crate) line: [u8; MAX_LINE_BYTES],
    pub(crate) line_len: usize,
    pub(crate) line_cursor: usize,
    pub(crate) history: [[u8; MAX_LINE_BYTES]; MAX_HISTORY],
    pub(crate) history_lens: [usize; MAX_HISTORY],
    pub(crate) history_count: usize,
    pub(crate) history_head: usize,
    pub(crate) history_view: Option<usize>,
    pub(crate) history_stash: [u8; MAX_LINE_BYTES],
    pub(crate) history_stash_len: usize,
    pub(crate) escape_state: EscapeState,
    pub(crate) occupied: bool,
}

impl Session {
    pub(crate) const fn empty() -> Self {
        Self {
            endpoint: rt::INVALID_HANDLE,
            id: 0,
            columns: DEFAULT_COLS,
            rows: DEFAULT_ROWS,
            width_pixels: 0,
            height_pixels: 0,
            line: [0; MAX_LINE_BYTES],
            line_len: 0,
            line_cursor: 0,
            history: [[0; MAX_LINE_BYTES]; MAX_HISTORY],
            history_lens: [0; MAX_HISTORY],
            history_count: 0,
            history_head: 0,
            history_view: None,
            history_stash: [0; MAX_LINE_BYTES],
            history_stash_len: 0,
            escape_state: EscapeState::None,
            occupied: false,
        }
    }
}
