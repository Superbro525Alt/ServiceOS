use serviceos_userspace_runtime as rt;

pub(crate) const MAX_SESSIONS: usize = 4;
pub(crate) const MAX_LINE_BYTES: usize = 128;
pub(crate) const MAX_HISTORY: usize = 16;
pub(crate) const MAX_INLINE_BYTES: usize = (rt::IPC_MAX_WORDS - 1) * 8;
pub(crate) const DEFAULT_COLS: u32 = 80;
pub(crate) const DEFAULT_ROWS: u32 = 25;
pub(crate) const MAX_PUBLIC_REQUESTS_PER_TURN: usize = 8;
pub(crate) const MAX_SESSION_MESSAGES_PER_TURN: usize = 16;

/// Retained output bytes per session; replayed to clients on reattach.
pub(crate) const SCROLLBACK_BYTES: usize = 4096;
/// Bookmarked command lines retained per session for quick re-edit.
pub(crate) const MAX_BOOKMARKS: usize = 8;

/// TCP port the remote-session listener binds (rsh-like plaintext protocol,
/// pre-SSH; must be allow-listed by the network firewall for non-loopback
/// peers). Knob lives here because the shared config-key ABI is frozen.
pub(crate) const REMOTE_LISTENER_PORT: u16 = 4023;
/// Accept backlog handed to the network listener.
pub(crate) const REMOTE_BACKLOG: u32 = 2;
/// Concurrent remote connections the service will bridge at once.
pub(crate) const MAX_REMOTE_LINKS: usize = 2;
/// Single-token auth gate for remote connections. EMPTY disables the gate:
/// any peer that reaches the port gets a shell. Plaintext by design — this
/// is honest pre-SSH groundwork, not a secure transport (S10 keeps SSH open).
pub(crate) const REMOTE_AUTH_TOKEN: &[u8] = b"";
/// Maximum payload bytes per framed chunk in either direction.
pub(crate) const REMOTE_FRAME_MAX: usize = 512;
/// Bounded retries when pumping sockets so one stuck link cannot starve the
/// main loop.
pub(crate) const REMOTE_PUMP_BUDGET: usize = 4;
/// Boot-time loopback self-connect probe. Default OFF, honestly: the
/// network service's TCP stack completes loopback handshakes for its own
/// internal raw-socket selftest, but a cross-service IPC-driven
/// connect-to-own-listener currently stalls before adoption (client SYN is
/// never picked up; verified by boot diagnostics). Unit-level bridge tests
/// cover the protocol instead until this lands alongside the SSH/TLS work.
pub(crate) const REMOTE_LOOPBACK_SELFTEST: bool = false;

/// Wire tags for the session persistence/bookmark extensions. Kept local so
/// the shared ABI enum stays untouched; values sit past TerminalTag::SessionClosed.
pub(crate) mod wire {
    pub(crate) const SESSION_ATTACH_REQUEST: u32 = 0xb10;
    pub(crate) const SESSION_ATTACH_REPLY: u32 = 0xb11;
    pub(crate) const SESSION_DETACH: u32 = 0xb13;
    pub(crate) const SESSION_BOOKMARK_ADD: u32 = 0xb14;
    pub(crate) const SESSION_BOOKMARK_CYCLE: u32 = 0xb15;
    pub(crate) const SESSION_ENUMERATE_REQUEST: u32 = 0xb16;
    pub(crate) const SESSION_ENUMERATE_REPLY: u32 = 0xb17;
    // Theme extensions: a client queries the service-global active theme
    // (GET pair, public channel) and mirrors operator picks per session
    // (SET, session channel). Values sit past 0xb17.
    pub(crate) const THEME_GET_REQUEST: u32 = 0xb18;
    pub(crate) const THEME_GET_REPLY: u32 = 0xb19;
    pub(crate) const THEME_SET: u32 = 0xb1a;
}

/// Number of named themes the terminal-app registry carries; indexes outside
/// this range are rejected rather than clamped.
pub(crate) const THEME_COUNT: usize = 6;
/// THEME_SET words[0] sentinel: clear the session override so the session
/// follows the service-global active theme again.
pub(crate) const THEME_CLEAR: u64 = 0xff;

/// Service-side theme state: one service-global active theme index plus an
/// optional per-session override on each session row. In memory only by
/// design — terminal-service holds no storage grant, so the durable operator
/// preference lives in the app's profiles.cfg store, which the app writes
/// before mirroring a pick here (graceful in-memory degrade, access.cfg
/// precedent).
#[derive(Clone, Copy)]
pub(crate) struct ThemeState {
    active: u8,
}

impl ThemeState {
    pub(crate) const fn new() -> Self {
        Self { active: 0 }
    }

    pub(crate) const fn active(&self) -> u8 {
        self.active
    }

    /// Set the service-global active theme. Rejects indexes past the
    /// registry (returns false, state unchanged).
    pub(crate) fn set_active(&mut self, index: u64) -> bool {
        if index as usize >= THEME_COUNT {
            return false;
        }
        self.active = index as u8;
        true
    }
}

/// Byte ring holding the most recent session output. Records continuously so
/// any later attach can restore the pane's visible history.
///
/// Resize/reflow policy: the ring is width-agnostic on purpose — it retains
/// the raw output stream, not a width-shaped grid, so a width change needs no
/// service-side transformation and no codec extension. The attaching app
/// re-wraps the replayed stream at its current pane width (terminal-app's
/// vt::reflow_pane owns the reflow semantics for live resizes and reattach).
#[derive(Clone, Copy)]
pub(crate) struct ScrollbackRing {
    bytes: [u8; SCROLLBACK_BYTES],
    head: usize,
    len: usize,
}

impl ScrollbackRing {
    pub(crate) const fn empty() -> Self {
        Self {
            bytes: [0; SCROLLBACK_BYTES],
            head: 0,
            len: 0,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }

    /// Append output bytes, evicting the oldest when over capacity.
    pub(crate) fn record(&mut self, input: &[u8]) {
        for chunk in input {
            self.bytes[self.head] = *chunk;
            self.head = (self.head + 1) % SCROLLBACK_BYTES;
            if self.len < SCROLLBACK_BYTES {
                self.len += 1;
            }
        }
    }

    /// Retained bytes oldest-first as at most two contiguous slices.
    pub(crate) fn slices(&self) -> (&[u8], &[u8]) {
        if self.len < SCROLLBACK_BYTES {
            return (&self.bytes[..self.head], &[]);
        }
        (&self.bytes[self.head..], &self.bytes[..self.head])
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn len(&self) -> usize {
        self.len
    }
}

/// Fixed-capacity list of bookmarked command lines with a cycle cursor for
/// quick re-edit traversal (newest first).
#[derive(Clone, Copy)]
pub(crate) struct BookmarkList {
    lines: [[u8; MAX_LINE_BYTES]; MAX_BOOKMARKS],
    lens: [usize; MAX_BOOKMARKS],
    count: usize,
    view: Option<usize>,
}

impl BookmarkList {
    pub(crate) const fn empty() -> Self {
        Self {
            lines: [[0; MAX_LINE_BYTES]; MAX_BOOKMARKS],
            lens: [0; MAX_BOOKMARKS],
            count: 0,
            view: None,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.count = 0;
        self.view = None;
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn count(&self) -> usize {
        self.count
    }

    /// Bookmark a command line. Duplicates of the newest entry are ignored;
    /// overflow evicts the oldest. Returns true when stored.
    pub(crate) fn add(&mut self, line: &[u8]) -> bool {
        if line.is_empty() || line.len() > MAX_LINE_BYTES {
            return false;
        }
        if self.count > 0 {
            let latest = self.count - 1;
            if self.lens[latest] == line.len() && &self.lines[latest][..line.len()] == line {
                return false;
            }
        }
        if self.count < MAX_BOOKMARKS {
            self.lines[self.count][..line.len()].copy_from_slice(line);
            self.lens[self.count] = line.len();
            self.count += 1;
        } else {
            self.lines.rotate_left(1);
            self.lens.rotate_left(1);
            self.lines[MAX_BOOKMARKS - 1][..line.len()].copy_from_slice(line);
            self.lens[MAX_BOOKMARKS - 1] = line.len();
        }
        self.view = None;
        true
    }

    /// Advance the cycle cursor (newest -> oldest -> newest) and copy the
    /// visited entry into `line`. Returns the copied length.
    pub(crate) fn cycle_next(&mut self, line: &mut [u8; MAX_LINE_BYTES]) -> Option<usize> {
        if self.count == 0 {
            return None;
        }
        let next = match self.view {
            None => self.count - 1,
            Some(0) => self.count - 1,
            Some(index) => index - 1,
        };
        self.view = Some(next);
        let len = self.lens[next];
        line[..len].copy_from_slice(&self.lines[next][..len]);
        Some(len)
    }

    pub(crate) fn reset_view(&mut self) {
        self.view = None;
    }
}

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
    field
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(field.len())
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
    /// Retained duplicate of the session channel used to mint client handles
    /// on reattach; stays valid for the whole session lifetime.
    pub(crate) spare_endpoint: rt::Handle,
    pub(crate) id: u32,
    pub(crate) columns: u32,
    pub(crate) rows: u32,
    pub(crate) width_pixels: u32,
    pub(crate) height_pixels: u32,
    /// True while a pane holds a client handle; false after a detach.
    pub(crate) attached: bool,
    /// Live remote-link stream handle while a TCP client bridges this
    /// session; INVALID_HANDLE otherwise. Output routes here (framed)
    /// instead of the pane endpoint whenever set.
    pub(crate) remote_stream: rt::Handle,
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
    pub(crate) scrollback: ScrollbackRing,
    pub(crate) bookmarks: BookmarkList,
    pub(crate) profile: SessionProfile,
    /// Per-session theme override (Some(index) while the pane picked its own
    /// theme); None means follow the service-global active theme.
    pub(crate) theme_override: Option<u8>,
    pub(crate) occupied: bool,
}

impl Session {
    pub(crate) const fn empty() -> Self {
        Self {
            endpoint: rt::INVALID_HANDLE,
            spare_endpoint: rt::INVALID_HANDLE,
            id: 0,
            columns: DEFAULT_COLS,
            rows: DEFAULT_ROWS,
            width_pixels: 0,
            height_pixels: 0,
            attached: false,
            remote_stream: rt::INVALID_HANDLE,
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
            scrollback: ScrollbackRing::empty(),
            bookmarks: BookmarkList::empty(),
            profile: SessionProfile::empty(),
            theme_override: None,
            occupied: false,
        }
    }

    /// Effective theme index for this session: its override when set, the
    /// service-global active theme otherwise.
    #[allow(dead_code)] // exercised by host tests; future surfaces read it too
    pub(crate) fn effective_theme(&self, themes: &ThemeState) -> u8 {
        self.theme_override.unwrap_or_else(|| themes.active())
    }

    /// Apply a THEME_SET to this session. words[0] = THEME_CLEAR drops the
    /// override (session follows global); a valid index sets the override
    /// AND mirrors the service-global active theme; any other value is
    /// rejected. Returns true when state changed.
    pub(crate) fn apply_theme_set(&mut self, themes: &mut ThemeState, value: u64) -> bool {
        if value == THEME_CLEAR {
            if self.theme_override.is_none() {
                return false;
            }
            self.theme_override = None;
            return true;
        }
        if value as usize >= THEME_COUNT {
            return false;
        }
        self.theme_override = Some(value as u8);
        themes.set_active(value);
        true
    }

    /// A detached session keeps its shell state but has no attached pane.
    pub(crate) fn is_detach_available(&self) -> bool {
        self.occupied && self.attached
    }

    /// Reattach preconditions: alive, currently detached, spare handle held.
    pub(crate) fn can_attach(&self) -> bool {
        self.occupied && !self.attached && self.spare_endpoint != rt::INVALID_HANDLE
    }

    /// Mark the session detached. Returns false when there is nothing to
    /// detach (free slot or already detached).
    pub(crate) fn mark_detached(&mut self) -> bool {
        if !self.is_detach_available() {
            return false;
        }
        self.attached = false;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_session() -> Session {
        let mut session = Session::empty();
        session.occupied = true;
        session.attached = true;
        session.spare_endpoint = 7;
        session
    }

    fn ring_bytes(ring: &ScrollbackRing) -> [u8; SCROLLBACK_BYTES] {
        let (first, second) = ring.slices();
        let mut joined = [0u8; SCROLLBACK_BYTES];
        joined[..first.len()].copy_from_slice(first);
        joined[first.len()..first.len() + second.len()].copy_from_slice(second);
        joined
    }

    #[test]
    fn scrollback_ring_retains_recent_output_in_order() {
        let mut ring = ScrollbackRing::empty();
        ring.record(b"alpha\r\n");
        ring.record(b"beta\r\n");
        let joined = ring_bytes(&ring);
        assert_eq!(&joined[..13], b"alpha\r\nbeta\r\n");
        assert_eq!(ring.len(), 13);
    }

    #[test]
    fn scrollback_ring_evicts_oldest_beyond_capacity() {
        let mut ring = ScrollbackRing::empty();
        // Write a sawtooth pattern twice the ring capacity.
        let total = SCROLLBACK_BYTES * 2;
        let mut chunk = [0u8; 64];
        let mut written = 0usize;
        while written < total {
            for slot in chunk.iter_mut() {
                *slot = (written % 251) as u8;
                written += 1;
            }
            ring.record(&chunk);
        }
        assert_eq!(ring.len(), SCROLLBACK_BYTES);
        let joined = ring_bytes(&ring);
        // Oldest byte retained is sequence position total - SCROLLBACK_BYTES.
        let start = total - SCROLLBACK_BYTES;
        for (index, byte) in joined.iter().enumerate() {
            assert_eq!(*byte, ((start + index) % 251) as u8);
        }
    }

    #[test]
    fn scrollback_ring_clear_resets() {
        let mut ring = ScrollbackRing::empty();
        ring.record(b"hello");
        ring.clear();
        assert_eq!(ring.len(), 0);
        assert_eq!(ring.slices().0.len() + ring.slices().1.len(), 0);
    }

    #[test]
    fn attach_detach_state_machine_transitions() {
        let mut session = sample_session();
        // Attached pane detaches: shell state survives, slot stays occupied.
        assert!(session.mark_detached());
        assert!(!session.attached && session.occupied);
        // Double detach is a no-op.
        assert!(!session.mark_detached());
        // Detached slot satisfies reattach preconditions.
        assert!(session.can_attach());
        session.attached = true;
        // Reattach over an attached session is refused.
        assert!(!session.can_attach());
        // Free slots are neither detachable nor attachable.
        let mut free = Session::empty();
        free.spare_endpoint = 7;
        assert!(!free.mark_detached());
        assert!(!free.can_attach());
        // Missing spare handle blocks reattach even when detached.
        let mut spareless = sample_session();
        spareless.spare_endpoint = rt::INVALID_HANDLE;
        spareless.attached = false;
        assert!(!spareless.can_attach());
    }

    #[test]
    fn bookmark_add_dedupes_and_cycles_newest_first_with_wraparound() {
        let mut bookmarks = BookmarkList::empty();
        assert!(!bookmarks.cycle_next(&mut [0u8; MAX_LINE_BYTES]).is_some());
        assert!(bookmarks.add(b"echo one"));
        assert!(!bookmarks.add(b"echo one"), "duplicate of newest ignored");
        assert!(bookmarks.add(b"echo two"));
        assert!(bookmarks.add(b"echo three"));
        assert_eq!(bookmarks.count(), 3);

        let mut line = [0u8; MAX_LINE_BYTES];
        let len = bookmarks.cycle_next(&mut line).expect("first cycle");
        assert_eq!(&line[..len], b"echo three", "newest first");
        let len = bookmarks.cycle_next(&mut line).expect("second cycle");
        assert_eq!(&line[..len], b"echo two");
        let len = bookmarks.cycle_next(&mut line).expect("third cycle");
        assert_eq!(&line[..len], b"echo one", "oldest last");
        let len = bookmarks.cycle_next(&mut line).expect("wraps around");
        assert_eq!(&line[..len], b"echo three");

        bookmarks.reset_view();
        let len = bookmarks
            .cycle_next(&mut line)
            .expect("reset restarts at newest");
        assert_eq!(&line[..len], b"echo three");
    }

    #[test]
    fn bookmark_overflow_evicts_oldest() {
        let mut bookmarks = BookmarkList::empty();
        for index in 0..MAX_BOOKMARKS + 2 {
            let text = format!("cmd-{index}");
            bookmarks.add(text.as_bytes());
        }
        assert_eq!(bookmarks.count(), MAX_BOOKMARKS);
        let mut line = [0u8; MAX_LINE_BYTES];
        let len = bookmarks.cycle_next(&mut line).expect("cycle");
        assert_eq!(&line[..len], b"cmd-9", "newest survives, cmd-0/1 evicted");
        let len = bookmarks.cycle_next(&mut line).expect("second entry");
        assert_eq!(&line[..len], b"cmd-8");
    }

    #[test]
    fn bookmark_rejects_empty_and_oversized_lines() {
        let mut bookmarks = BookmarkList::empty();
        assert!(!bookmarks.add(b""));
        let oversized = [b'x'; MAX_LINE_BYTES + 1];
        assert!(!bookmarks.add(&oversized));
        assert_eq!(bookmarks.count(), 0);
    }
}

#[cfg(test)]
mod theme_tests {
    use super::*;

    #[test]
    fn theme_state_starts_on_default_and_rejects_out_of_registry() {
        let mut themes = ThemeState::new();
        assert_eq!(themes.active(), 0);
        assert!(themes.set_active(5));
        assert_eq!(themes.active(), 5);
        assert!(!themes.set_active(THEME_COUNT as u64));
        assert_eq!(themes.active(), 5, "rejected set leaves state unchanged");
        assert!(!themes.set_active(u64::MAX));
    }

    #[test]
    fn session_override_set_clear_and_fallback() {
        let mut themes = ThemeState::new();
        let mut session = Session::empty();
        session.occupied = true;
        assert_eq!(session.effective_theme(&themes), 0, "no override: global");

        assert!(session.apply_theme_set(&mut themes, 3));
        assert_eq!(session.theme_override, Some(3));
        assert_eq!(session.effective_theme(&themes), 3);
        assert_eq!(themes.active(), 3, "pick mirrors service-global");

        assert!(session.apply_theme_set(&mut themes, THEME_CLEAR));
        assert_eq!(session.theme_override, None);
        assert_eq!(
            session.effective_theme(&themes),
            3,
            "follows global after clear"
        );

        assert!(!session.apply_theme_set(&mut themes, THEME_COUNT as u64));
        assert_eq!(session.theme_override, None, "invalid index rejected");

        assert!(
            !session.apply_theme_set(&mut themes, THEME_CLEAR),
            "clear twice: no-op"
        );
    }

    #[test]
    fn theme_wire_tags_are_additive_and_distinct() {
        assert_eq!(wire::THEME_GET_REQUEST, 0xb18);
        assert_eq!(wire::THEME_GET_REPLY, 0xb19);
        assert_eq!(wire::THEME_SET, 0xb1a);
        let existing = [
            wire::SESSION_ATTACH_REQUEST,
            wire::SESSION_ATTACH_REPLY,
            wire::SESSION_DETACH,
            wire::SESSION_BOOKMARK_ADD,
            wire::SESSION_BOOKMARK_CYCLE,
            wire::SESSION_ENUMERATE_REQUEST,
            wire::SESSION_ENUMERATE_REPLY,
        ];
        for tag in existing {
            assert_ne!(tag, wire::THEME_GET_REQUEST);
            assert_ne!(tag, wire::THEME_GET_REPLY);
            assert_ne!(tag, wire::THEME_SET);
        }
    }
}
