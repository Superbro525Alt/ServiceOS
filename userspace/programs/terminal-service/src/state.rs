use serviceos_userspace_runtime as rt;

pub(crate) const MAX_SESSIONS: usize = 4;
pub(crate) const MAX_LINE_BYTES: usize = 128;
pub(crate) const MAX_HISTORY: usize = 16;
pub(crate) const MAX_INLINE_BYTES: usize = (rt::IPC_MAX_WORDS - 1) * 8;
pub(crate) const DEFAULT_COLS: u32 = 80;
pub(crate) const DEFAULT_ROWS: u32 = 25;
pub(crate) const MAX_PUBLIC_REQUESTS_PER_TURN: usize = 8;
pub(crate) const MAX_SESSION_MESSAGES_PER_TURN: usize = 16;

/// Launch metadata relayed by terminal-app session profiles
/// (name/program/args/env/cwd). Mirrors the app-side wire layout.
pub(crate) const PROFILE_NAME_BYTES: usize = 10;
pub(crate) const PROFILE_PROGRAM_BYTES: usize = 18;
pub(crate) const PROFILE_ARGS_BYTES: usize = 22;
pub(crate) const PROFILE_ENV_BYTES: usize = 36;
pub(crate) const PROFILE_CWD_BYTES: usize = 22;
pub(crate) const PROFILE_WIRE_LEN: usize = PROFILE_NAME_BYTES
    + PROFILE_PROGRAM_BYTES
    + PROFILE_ARGS_BYTES
    + PROFILE_ENV_BYTES
    + PROFILE_CWD_BYTES
    + 1;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct SessionProfile {
    pub(crate) name: [u8; PROFILE_NAME_BYTES],
    pub(crate) name_len: usize,
    pub(crate) program: [u8; PROFILE_PROGRAM_BYTES],
    pub(crate) program_len: usize,
    pub(crate) args: [u8; PROFILE_ARGS_BYTES],
    pub(crate) args_len: usize,
    pub(crate) env: [u8; PROFILE_ENV_BYTES],
    pub(crate) env_len: usize,
    pub(crate) cwd: [u8; PROFILE_CWD_BYTES],
    pub(crate) cwd_len: usize,
}

impl SessionProfile {
    pub(crate) const fn empty() -> Self {
        Self {
            name: [0; PROFILE_NAME_BYTES],
            name_len: 0,
            program: [0; PROFILE_PROGRAM_BYTES],
            program_len: 0,
            args: [0; PROFILE_ARGS_BYTES],
            args_len: 0,
            env: [0; PROFILE_ENV_BYTES],
            env_len: 0,
            cwd: [0; PROFILE_CWD_BYTES],
            cwd_len: 0,
        }
    }

    pub(crate) fn from_wire(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < PROFILE_WIRE_LEN {
            return None;
        }
        let mut profile = Self::empty();
        let mut offset = 0usize;
        fn take<const N: usize>(source: &[u8], offset: &mut usize) -> [u8; N] {
            let mut out = [0u8; N];
            out.copy_from_slice(&source[*offset..*offset + N]);
            *offset += N;
            out
        }
        profile.name = take::<PROFILE_NAME_BYTES>(bytes, &mut offset);
        profile.program = take::<PROFILE_PROGRAM_BYTES>(bytes, &mut offset);
        profile.args = take::<PROFILE_ARGS_BYTES>(bytes, &mut offset);
        profile.env = take::<PROFILE_ENV_BYTES>(bytes, &mut offset);
        profile.cwd = take::<PROFILE_CWD_BYTES>(bytes, &mut offset);
        // Last byte is the theme index, consumed by the app only.
        profile.name_len = cstr_len(&profile.name);
        profile.program_len = cstr_len(&profile.program);
        profile.args_len = cstr_len(&profile.args);
        profile.env_len = cstr_len(&profile.env);
        profile.cwd_len = cstr_len(&profile.cwd);
        Some(profile)
    }
}

fn cstr_len(field: &[u8]) -> usize {
    field.iter().position(|byte| *byte == 0).unwrap_or(field.len())
}

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
    pub(crate) profile: SessionProfile,
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
        profile: SessionProfile::empty(),
        occupied: false,
    }
    }
}
