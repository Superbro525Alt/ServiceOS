use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;

pub(crate) const BUFFER_WIDTH: u32 = 1024;
pub(crate) const BUFFER_HEIGHT: u32 = 768;
pub(crate) const BUFFER_BYTES: usize = BUFFER_WIDTH as usize * BUFFER_HEIGHT as usize * 4;
pub(crate) const SURFACE_BUFFER_SLOTS: usize = 2;
pub(crate) const PIXEL_STRIDE: usize = BUFFER_WIDTH as usize;
pub(crate) const MAX_STORAGE_PATH: usize = 96;
pub(crate) const MAX_ENTRIES: usize = 64;
pub(crate) const LIST_X: usize = 12;
pub(crate) const LIST_Y: usize = ui::TITLEBAR_HEIGHT as usize + 34;
pub(crate) const LIST_BOTTOM_MARGIN: usize = 18;
pub(crate) const ROW_HEIGHT: usize = 14;
pub(crate) const KEY_BACKSPACE: u32 = 14;
pub(crate) const KEY_ENTER: u32 = 28;
pub(crate) const KEY_ESC: u32 = 1;
pub(crate) const KEY_O: u32 = 18;
pub(crate) const KEY_D: u32 = 32;
pub(crate) const KEY_R: u32 = 19;
pub(crate) const KEY_UP: u32 = 103;
pub(crate) const KEY_PAGE_UP: u32 = 104;
pub(crate) const KEY_LEFT: u32 = 105;
pub(crate) const KEY_RIGHT: u32 = 106;
pub(crate) const KEY_DOWN: u32 = 108;
pub(crate) const KEY_PAGE_DOWN: u32 = 109;
pub(crate) const MOD_SHIFT: u32 = 1 << 0;
/// Pointer travel (px, either axis) that turns a press on a file row into a
/// drag gesture.
pub(crate) const DRAG_THRESHOLD_PX: i32 = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EntryKind {
    Parent,
    Directory,
    File,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ViewMode {
    Directory,
    Recent,
}

#[derive(Clone, Copy)]
pub(crate) struct Press {
    pub(crate) index: usize,
    pub(crate) x: i32,
    pub(crate) y: i32,
}

#[derive(Clone, Copy)]
pub(crate) struct ExplorerEntry {
    pub(crate) kind: EntryKind,
    pub(crate) path: [u8; MAX_STORAGE_PATH],
    pub(crate) path_len: usize,
}

impl ExplorerEntry {
    pub(crate) const fn empty() -> Self {
        Self {
            kind: EntryKind::File,
            path: [0; MAX_STORAGE_PATH],
            path_len: 0,
        }
    }
}

pub(crate) struct ExplorerState {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) focused: bool,
    pub(crate) loading_initial_directory: bool,
    pub(crate) current_directory_handle: rt::Handle,
    pub(crate) current_path: [u8; MAX_STORAGE_PATH],
    pub(crate) current_path_len: usize,
    pub(crate) entries: [ExplorerEntry; MAX_ENTRIES],
    pub(crate) entry_count: usize,
    pub(crate) selected_index: usize,
    pub(crate) scroll_offset: usize,
    pub(crate) load_failed: bool,
    /// Recent-files view toggle.
    pub(crate) view_mode: ViewMode,
    pub(crate) recent_sel: usize,
    /// Pending press on a file row that may grow into a drag gesture.
    pub(crate) press: Option<Press>,
    pub(crate) dragging: bool,
    /// Open-with candidate index for the selected file (None = default).
    pub(crate) open_with_pick: Option<usize>,
    pub(crate) assoc: crate::assoc::AssocTable,
    pub(crate) recent: crate::recent::RecentRing,
    /// Writable store directory handle (INVALID_HANDLE = persistence off).
    pub(crate) persist_dir: rt::Handle,
}
