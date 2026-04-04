use serviceos_userspace_runtime as rt;

pub(crate) const MAX_SESSIONS: usize = 2;
pub(crate) const MAX_LINE_BYTES: usize = 128;
pub(crate) const MAX_DISPLAY_BYTES: usize = 192;
pub(crate) const MAX_HISTORY: usize = 16;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum EscapeState {
    None,
    Esc,
    Csi,
}

#[derive(Clone, Copy)]
pub(crate) struct Session {
    pub(crate) endpoint: rt::Handle,
    pub(crate) pending_reply: rt::Handle,
    pub(crate) line: [u8; MAX_LINE_BYTES],
    pub(crate) line_len: usize,
    pub(crate) line_cursor: usize,
    pub(crate) display: [u8; MAX_DISPLAY_BYTES],
    pub(crate) display_len: usize,
    pub(crate) prompt_len: usize,
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
            pending_reply: rt::INVALID_HANDLE,
            line: [0; MAX_LINE_BYTES],
            line_len: 0,
            line_cursor: 0,
            display: [0; MAX_DISPLAY_BYTES],
            display_len: 0,
            prompt_len: 0,
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

pub(crate) fn active_session(sessions: &[Session; MAX_SESSIONS]) -> Option<&Session> {
    sessions.iter().find(|session| {
        session.occupied
            && session.pending_reply != rt::INVALID_HANDLE
            && session.display_len > 0
    })
}

pub(crate) fn release_session(session: &mut Session) {
    let endpoint = session.endpoint;
    reset_input_state(session);
    if endpoint != rt::INVALID_HANDLE {
        let _ = rt::handle_close(endpoint);
    }
    *session = Session::empty();
}

pub(crate) fn begin_input_session(session: &mut Session) {
    session.line_len = 0;
    session.line_cursor = 0;
    session.prompt_len = session.display_len;
    session.history_view = None;
    session.history_stash_len = 0;
    session.escape_state = EscapeState::None;
}

pub(crate) fn reset_input_state(session: &mut Session) {
    if session.pending_reply != rt::INVALID_HANDLE {
        let _ = rt::handle_close(session.pending_reply);
    }
    session.pending_reply = rt::INVALID_HANDLE;
    session.line_len = 0;
    session.line_cursor = 0;
    session.prompt_len = 0;
    session.display_len = 0;
    session.history_view = None;
    session.history_stash_len = 0;
    session.escape_state = EscapeState::None;
}

pub(crate) fn push_display_byte(session: &mut Session, byte: u8) {
    if matches!(byte, 0x20..=0x7e) && session.display_len < session.display.len() {
        session.display[session.display_len] = byte;
        session.display_len += 1;
    }
}

pub(crate) fn pop_display_byte(session: &mut Session) {
    if session.display_len > 0 {
        session.display_len -= 1;
    }
}
