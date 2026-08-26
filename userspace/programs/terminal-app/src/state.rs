use core::cell::UnsafeCell;

use crate::profiles::TerminalProfile;
use serviceos_userspace_runtime as rt;

pub(crate) const BUFFER_WIDTH: u32 = 1024;
pub(crate) const BUFFER_HEIGHT: u32 = 768;
pub(crate) const BUFFER_BYTES: usize = BUFFER_WIDTH as usize * BUFFER_HEIGHT as usize * 4;
pub(crate) const SURFACE_BUFFER_SLOTS: usize = 2;
pub(crate) const MAX_TABS: usize = 4;
pub(crate) const MAX_PANES_PER_TAB: usize = 2;
pub(crate) const MAX_COLS: usize = 120;
pub(crate) const MAX_SCROLLBACK_LINES: usize = 256;
pub(crate) const MAX_OUTPUT_MESSAGES_PER_PANE_PER_TURN: usize = 8;
pub(crate) const CLIPBOARD_BYTES: usize = 1024;
pub(crate) const MAX_TITLE_BYTES: usize = 24;
pub(crate) const MAX_OSC_BYTES: usize = 64;
pub(crate) const CELL_WIDTH: usize = 6;
pub(crate) const CELL_HEIGHT: usize = 10;
pub(crate) const CONTENT_PADDING_X: usize = 10;
pub(crate) const CONTENT_PADDING_Y: usize = 8;
pub(crate) const TAB_STRIP_HEIGHT: usize = 18;
pub(crate) const TAB_WIDTH: usize = 100;
pub(crate) const KEY_1: u32 = 2;
pub(crate) const KEY_2: u32 = 3;
pub(crate) const KEY_3: u32 = 4;
pub(crate) const KEY_ESC: u32 = 1;
pub(crate) const KEY_BACKSPACE: u32 = 14;
pub(crate) const KEY_TAB: u32 = 15;
pub(crate) const KEY_Q: u32 = 16;
pub(crate) const KEY_W: u32 = 17;
pub(crate) const KEY_E: u32 = 18;
pub(crate) const KEY_R: u32 = 19;
pub(crate) const KEY_T: u32 = 20;
pub(crate) const KEY_P: u32 = 25;
pub(crate) const KEY_D: u32 = 32;
pub(crate) const KEY_B: u32 = 48;
pub(crate) const KEY_C: u32 = 46;
pub(crate) const KEY_V: u32 = 47;
pub(crate) const KEY_UP: u32 = 103;
pub(crate) const KEY_PAGE_UP: u32 = 104;
pub(crate) const KEY_LEFT: u32 = 105;
pub(crate) const KEY_RIGHT: u32 = 106;
pub(crate) const KEY_DOWN: u32 = 108;
pub(crate) const KEY_PAGE_DOWN: u32 = 109;
pub(crate) const MOD_SHIFT: u32 = 1 << 0;
pub(crate) const MOD_ALT: u32 = 1 << 1;
pub(crate) const MOD_CTRL: u32 = 1 << 2;
pub(crate) const PIXEL_STRIDE: usize = BUFFER_WIDTH as usize;

/// Wire tags mirroring terminal-service's local session-persistence
/// extensions (values past TerminalTag::SessionClosed).
pub(crate) mod wire {
    pub(crate) const SESSION_ATTACH_REQUEST: u32 = 0xb10;
    pub(crate) const SESSION_ATTACH_REPLY: u32 = 0xb11;
    pub(crate) const SESSION_DETACH: u32 = 0xb13;
    pub(crate) const SESSION_BOOKMARK_ADD: u32 = 0xb14;
    pub(crate) const SESSION_BOOKMARK_CYCLE: u32 = 0xb15;
    pub(crate) const SESSION_ENUMERATE_REQUEST: u32 = 0xb16;
    pub(crate) const SESSION_ENUMERATE_REPLY: u32 = 0xb17;
}

pub(crate) const COLOR_DEFAULT: u8 = 0;
pub(crate) const CELL_FLAG_BOLD: u8 = 1 << 0;
pub(crate) const CELL_FLAG_INVERSE: u8 = 1 << 1;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct Cell {
    pub(crate) ch: u8,
    pub(crate) fg: u8,
    pub(crate) bg: u8,
    pub(crate) flags: u8,
}

impl Cell {
    pub(crate) const fn blank() -> Self {
        Self {
            ch: b' ',
            fg: COLOR_DEFAULT,
            bg: COLOR_DEFAULT,
            flags: 0,
        }
    }
}

/// Grid slots are keyed by pane, not tab: slot = tab * MAX_PANES_PER_TAB + pane.
pub(crate) const fn grid_slot(tab_index: usize, pane_index: usize) -> usize {
    tab_index * MAX_PANES_PER_TAB + pane_index
}

pub(crate) const GRID_SLOTS: usize = MAX_TABS * MAX_PANES_PER_TAB;

pub(crate) struct GlobalCells(UnsafeCell<[[[Cell; MAX_COLS]; MAX_SCROLLBACK_LINES]; GRID_SLOTS]>);
pub(crate) struct GlobalWraps(UnsafeCell<[[bool; MAX_SCROLLBACK_LINES]; GRID_SLOTS]>);
pub(crate) struct ReflowCells(UnsafeCell<[[Cell; MAX_COLS]; MAX_SCROLLBACK_LINES]>);
pub(crate) struct ReflowWraps(UnsafeCell<[bool; MAX_SCROLLBACK_LINES]>);

unsafe impl Sync for GlobalCells {}
unsafe impl Sync for GlobalWraps {}
unsafe impl Sync for ReflowCells {}
unsafe impl Sync for ReflowWraps {}

impl GlobalCells {
    pub(crate) const fn new() -> Self {
        Self(UnsafeCell::new(
            [[[Cell::blank(); MAX_COLS]; MAX_SCROLLBACK_LINES]; GRID_SLOTS],
        ))
    }

    pub(crate) unsafe fn pane(&self, slot: usize) -> &[[Cell; MAX_COLS]; MAX_SCROLLBACK_LINES] {
        unsafe { &(*self.0.get())[slot] }
    }

    pub(crate) unsafe fn pane_mut(
        &self,
        slot: usize,
    ) -> &mut [[Cell; MAX_COLS]; MAX_SCROLLBACK_LINES] {
        unsafe { &mut (*self.0.get())[slot] }
    }
}

impl GlobalWraps {
    pub(crate) const fn new() -> Self {
        Self(UnsafeCell::new([[false; MAX_SCROLLBACK_LINES]; GRID_SLOTS]))
    }

    pub(crate) unsafe fn wraps_mut(&self, slot: usize) -> &mut [bool; MAX_SCROLLBACK_LINES] {
        unsafe { &mut (*self.0.get())[slot] }
    }
}

impl ReflowCells {
    pub(crate) const fn new() -> Self {
        Self(UnsafeCell::new(
            [[Cell::blank(); MAX_COLS]; MAX_SCROLLBACK_LINES],
        ))
    }

    pub(crate) unsafe fn get(&self) -> &mut [[Cell; MAX_COLS]; MAX_SCROLLBACK_LINES] {
        unsafe { &mut *self.0.get() }
    }
}

impl ReflowWraps {
    pub(crate) const fn new() -> Self {
        Self(UnsafeCell::new([false; MAX_SCROLLBACK_LINES]))
    }

    pub(crate) unsafe fn get(&self) -> &mut [bool; MAX_SCROLLBACK_LINES] {
        unsafe { &mut *self.0.get() }
    }
}

pub(crate) static GRIDS: GlobalCells = GlobalCells::new();
pub(crate) static WRAPS: GlobalWraps = GlobalWraps::new();
pub(crate) static REFLOW_CELLS: ReflowCells = ReflowCells::new();
pub(crate) static REFLOW_WRAPS: ReflowWraps = ReflowWraps::new();

#[derive(Clone, Copy)]
pub(crate) struct Theme {
    pub(crate) name: &'static str,
    pub(crate) bg: u32,
    pub(crate) panel: u32,
    pub(crate) panel_alt: u32,
    pub(crate) fg: u32,
    pub(crate) muted: u32,
    pub(crate) selection: u32,
    pub(crate) ansi: [u32; 16],
}

pub(crate) const THEMES: [Theme; 3] = [
    Theme {
        name: "MIDNIGHT",
        bg: 0x0b1220,
        panel: 0x10151d,
        panel_alt: 0x122035,
        fg: 0xe6edf5,
        muted: 0x8fa4ba,
        selection: 0x23496f,
        ansi: [
            0x0b1220, 0xd05858, 0x65b35c, 0xd1af47, 0x5d8bd6, 0xb470d0, 0x57b8c4, 0xc7d3df,
            0x405469, 0xff8b8b, 0x8ce17f, 0xf4d46f, 0x89b4ff, 0xd7a7ff, 0x7de2ef, 0xf8fbff,
        ],
    },
    Theme {
        name: "PAPER",
        bg: 0xf2efe8,
        panel: 0xe7e0d6,
        panel_alt: 0xd8d0c3,
        fg: 0x1f242a,
        muted: 0x61686f,
        selection: 0xbfd7ff,
        ansi: [
            0xf2efe8, 0xb53c3c, 0x3f8d3c, 0xa76d10, 0x2e63ad, 0x8a47a6, 0x287d82, 0x3f474f,
            0xa89f92, 0xd95a5a, 0x52ad4f, 0xc88d1f, 0x447fd4, 0xa663c4, 0x3b9fa5, 0x101316,
        ],
    },
    Theme {
        name: "AMBER",
        bg: 0x140f08,
        panel: 0x20170d,
        panel_alt: 0x2a1d0d,
        fg: 0xf0d0a2,
        muted: 0xb59363,
        selection: 0x5b3a12,
        ansi: [
            0x140f08, 0xc35b4c, 0x9ea95b, 0xe0a14a, 0x7e90c4, 0xb986c8, 0x6ca2b8, 0xf0d0a2,
            0x6b5135, 0xe48a73, 0xc6d47d, 0xffc46b, 0x9fb3e8, 0xd3a5df, 0x88bfd4, 0xffefd0,
        ],
    },
];

#[derive(Clone, Copy)]
pub(crate) struct TerminalState {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) focused: bool,
    pub(crate) terminal_handle: rt::Handle,
    pub(crate) clipboard_handle: rt::Handle,
    /// Storage-service channel for durable profile persistence; stays
    /// INVALID_HANDLE when lookup failed (profiles then run in-memory only).
    pub(crate) storage_handle: rt::Handle,
    pub(crate) columns: usize,
    pub(crate) rows: usize,
    pub(crate) content_x: usize,
    pub(crate) content_y: usize,
    pub(crate) content_w: usize,
    pub(crate) content_h: usize,
    pub(crate) active_tab: usize,
    pub(crate) theme_index: usize,
    pub(crate) profile_index: usize,
    /// Working profile set: defaults overlaid by the persisted store, written
    /// back whenever the operator picks a new theme.
    pub(crate) profiles: [TerminalProfile; crate::profiles::PROFILE_COUNT],
    /// Active Ctrl-R reverse history search overlay, if any.
    pub(crate) search: Option<SearchOverlay>,
    pub(crate) tabs: [TerminalTab; MAX_TABS],
    pub(crate) selection: Option<Selection>,
    pub(crate) clipboard: [u8; CLIPBOARD_BYTES],
    pub(crate) clipboard_len: usize,
}

/// Ctrl-R state bound to one pane of the active tab.
#[derive(Clone, Copy)]
pub(crate) struct SearchOverlay {
    pub(crate) pane_index: usize,
    pub(crate) inner: serviceos_shell_service::history_search::HistorySearch,
}

#[derive(Clone, Copy)]
pub(crate) struct TerminalTab {
    pub(crate) occupied: bool,
    pub(crate) profile_index: usize,
    pub(crate) pane_count: usize,
    pub(crate) tree: crate::panes::PaneTree,
    pub(crate) panes: [TerminalPane; MAX_PANES_PER_TAB],
}

impl TerminalTab {
    pub(crate) const fn empty() -> Self {
        Self {
            occupied: false,
            profile_index: 0,
            pane_count: 0,
            tree: crate::panes::PaneTree::single(),
            panes: [TerminalPane::empty(); MAX_PANES_PER_TAB],
        }
    }

    pub(crate) fn focused_pane_mut(&mut self) -> Option<&mut TerminalPane> {
        if !self.occupied {
            return None;
        }
        self.panes
            .get_mut(self.tree.focused.min(self.pane_count - 1))
    }

    pub(crate) fn focused_pane_ref(&self) -> Option<&TerminalPane> {
        if !self.occupied {
            return None;
        }
        self.panes.get(self.tree.focused.min(self.pane_count - 1))
    }
}

#[derive(Clone, Copy)]
pub(crate) struct TerminalPane {
    pub(crate) session_handle: rt::Handle,
    pub(crate) session_id: u32,
    pub(crate) columns: usize,
    pub(crate) rows: usize,
    pub(crate) line_count: usize,
    pub(crate) cursor_line: usize,
    pub(crate) cursor_col: usize,
    pub(crate) saved_cursor_line: usize,
    pub(crate) saved_cursor_col: usize,
    pub(crate) scroll_offset: usize,
    pub(crate) parse_state: ParseState,
    pub(crate) csi_params: [usize; 8],
    pub(crate) csi_count: usize,
    pub(crate) csi_private: bool,
    pub(crate) osc_bytes: [u8; MAX_OSC_BYTES],
    pub(crate) osc_len: usize,
    pub(crate) osc_esc_pending: bool,
    pub(crate) title: [u8; MAX_TITLE_BYTES],
    pub(crate) title_len: usize,
    pub(crate) current_fg: u8,
    pub(crate) current_bg: u8,
    pub(crate) current_flags: u8,
    pub(crate) cursor_visible: bool,
    /// Local echo of the service-side editable line (typed chars, backspaces,
    /// arrow recalls) so the app can run Ctrl-R over this pane's commands.
    pub(crate) input_mirror: [u8; MIRROR_LINE_BYTES],
    pub(crate) input_mirror_len: usize,
    /// Mirror of the service-side history navigation view.
    pub(crate) hist_view: Option<usize>,
    pub(crate) hist_stash: [u8; MIRROR_LINE_BYTES],
    pub(crate) hist_stash_len: usize,
    /// Commands submitted in this pane, newest last; the Ctrl-R corpus.
    pub(crate) history: PaneHistory,
}

/// Bounded per-pane command ring backing reverse search. Newest-last order
/// indexing matches the shell session rings so `HistorySource` applies.
#[derive(Clone, Copy)]
pub(crate) struct PaneHistory {
    entries: [[u8; PANE_HISTORY_LINE_BYTES]; PANE_HISTORY_ENTRIES],
    lens: [usize; PANE_HISTORY_ENTRIES],
    count: usize,
    head: usize,
}

pub(crate) const PANE_HISTORY_ENTRIES: usize = 16;
pub(crate) const PANE_HISTORY_LINE_BYTES: usize = 96;
pub(crate) const MIRROR_LINE_BYTES: usize = 96;

impl PaneHistory {
    pub(crate) const fn new() -> Self {
        Self {
            entries: [[0; PANE_HISTORY_LINE_BYTES]; PANE_HISTORY_ENTRIES],
            lens: [0; PANE_HISTORY_ENTRIES],
            count: 0,
            head: 0,
        }
    }

    /// Record one submitted line; consecutive duplicates collapse.
    pub(crate) fn push(&mut self, line: &[u8]) {
        let len = line.len().min(PANE_HISTORY_LINE_BYTES);
        let line = &line[..len];
        if line.is_empty() {
            return;
        }
        let slot = self.latest_slot();
        if self.count > 0 && self.lens[slot] == len && &self.entries[slot][..len] == line {
            return;
        }
        self.entries[self.head][..len].copy_from_slice(line);
        self.lens[self.head] = len;
        self.head = (self.head + 1) % PANE_HISTORY_ENTRIES;
        if self.count < PANE_HISTORY_ENTRIES {
            self.count += 1;
        }
    }

    fn latest_slot(&self) -> usize {
        (self.head + PANE_HISTORY_ENTRIES - 1) % PANE_HISTORY_ENTRIES
    }

    fn slot(&self, order: usize) -> usize {
        (self.head + PANE_HISTORY_ENTRIES - self.count + order) % PANE_HISTORY_ENTRIES
    }
}

impl serviceos_shell_service::history_search::HistorySource for PaneHistory {
    fn count(&self) -> usize {
        self.count
    }

    fn entry(&self, order: usize, out: &mut [u8]) -> Option<usize> {
        if order >= self.count {
            return None;
        }
        let slot = self.slot(order);
        let len = self.lens[slot].min(out.len());
        out[..len].copy_from_slice(&self.entries[slot][..len]);
        Some(len)
    }
}

impl TerminalPane {
    pub(crate) const fn empty() -> Self {
        Self {
            session_handle: rt::INVALID_HANDLE,
            session_id: 0,
            columns: 0,
            rows: 0,
            line_count: 1,
            cursor_line: 0,
            cursor_col: 0,
            saved_cursor_line: 0,
            saved_cursor_col: 0,
            scroll_offset: 0,
            parse_state: ParseState::Ground,
            csi_params: [0; 8],
            csi_count: 0,
            csi_private: false,
            osc_bytes: [0; MAX_OSC_BYTES],
            osc_len: 0,
            osc_esc_pending: false,
            title: [0; MAX_TITLE_BYTES],
            title_len: 0,
            current_fg: COLOR_DEFAULT,
            current_bg: COLOR_DEFAULT,
            current_flags: 0,
            cursor_visible: true,
            input_mirror: [0; MIRROR_LINE_BYTES],
            input_mirror_len: 0,
            hist_view: None,
            hist_stash: [0; MIRROR_LINE_BYTES],
            hist_stash_len: 0,
            history: PaneHistory::new(),
        }
    }

    pub(crate) fn opened(session_handle: rt::Handle, session_id: u32) -> Self {
        let mut pane = Self::empty();
        pane.session_handle = session_handle;
        pane.session_id = session_id;
        pane
    }

    /// Mirror contents with surrounding whitespace trimmed.
    pub(crate) fn trimmed_mirror(&self) -> &[u8] {
        let end = self.input_mirror_len.min(self.input_mirror.len());
        let slice = &self.input_mirror[..end];
        let start = slice
            .iter()
            .position(|byte| *byte != b' ' && *byte != b'\t')
            .unwrap_or(end);
        let end = slice
            [start..]
            .iter()
            .rposition(|byte| *byte != b' ' && *byte != b'\t')
            .map_or(start, |offset| start + offset + 1);
        &slice[start..end]
    }

    /// Replace the mirror with an externally chosen line (search accept) and
    /// clear the recall navigation state.
    pub(crate) fn mirror_reset(&mut self, line: &[u8]) {
        let len = line.len().min(MIRROR_LINE_BYTES);
        self.input_mirror = [0; MIRROR_LINE_BYTES];
        self.input_mirror[..len].copy_from_slice(&line[..len]);
        self.input_mirror_len = len;
        self.hist_view = None;
        self.hist_stash_len = 0;
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum ParseState {
    Ground,
    Esc,
    Csi,
    Osc,
}

#[derive(Clone, Copy)]
pub(crate) struct CellPos {
    pub(crate) line: usize,
    pub(crate) col: usize,
}

#[derive(Clone, Copy)]
pub(crate) struct Selection {
    pub(crate) pane: usize,
    pub(crate) anchor: CellPos,
    pub(crate) focus: CellPos,
    pub(crate) dragging: bool,
}
