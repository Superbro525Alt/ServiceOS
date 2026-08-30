use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;

pub(crate) const BUFFER_WIDTH: u32 = 1024;
pub(crate) const BUFFER_HEIGHT: u32 = 768;
pub(crate) const BUFFER_BYTES: usize = BUFFER_WIDTH as usize * BUFFER_HEIGHT as usize * 4;
pub(crate) const SURFACE_BUFFER_SLOTS: usize = 2;
pub(crate) const PIXEL_STRIDE: usize = BUFFER_WIDTH as usize;
pub(crate) const MAX_STORAGE_PATH: usize = 96;
pub(crate) const MAX_ENTRIES: usize = 64;
pub(crate) const MAX_SEARCH_QUERY: usize = rt::STORAGE_SEARCH_QUERY_BYTES_MAX;
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
pub(crate) const KEY_N: u32 = 49;
pub(crate) const KEY_F2: u32 = 60;
pub(crate) const KEY_DELETE: u32 = 111;
pub(crate) const KEY_UP: u32 = 103;
pub(crate) const KEY_PAGE_UP: u32 = 104;
pub(crate) const KEY_LEFT: u32 = 105;
pub(crate) const KEY_RIGHT: u32 = 106;
pub(crate) const KEY_DOWN: u32 = 108;
pub(crate) const KEY_PAGE_DOWN: u32 = 109;
pub(crate) const MOD_SHIFT: u32 = 1 << 0;
pub(crate) const MOD_CTRL: u32 = 1 << 2;
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
    Search,
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
    /// Bounded directory-scoped name-search text.
    pub(crate) search_query: [u8; MAX_SEARCH_QUERY],
    pub(crate) search_query_len: usize,
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
    /// Modal operation dialog (confirm/prompt/error/progress).
    pub(crate) dialog: Option<Dialog>,
    /// Typed characters for the active prompt.
    pub(crate) prompt_input: [u8; crate::ops::NAME_MAX],
    pub(crate) prompt_len: usize,
    /// Open context menu: (entry index, highlighted action cursor).
    pub(crate) menu: Option<(usize, usize)>,
    /// Row awaiting a second click that would open its context menu.
    pub(crate) await_context: Option<usize>,
}

/// What the typed prompt will do when committed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PromptPurpose {
    Rename,
    NewFolder,
    NewFile,
    MoveTo,
}

impl PromptPurpose {
    pub(crate) fn title(self) -> &'static str {
        match self {
            PromptPurpose::Rename => "RENAME TO:",
            PromptPurpose::NewFolder => "NEW FOLDER:",
            PromptPurpose::NewFile => "NEW FILE:",
            PromptPurpose::MoveTo => "MOVE TO DIR:",
        }
    }
}

/// Modal operation dialog driving keyboard-first flows with pointer
/// equivalents.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Dialog {
    /// ENTER confirms deleting entry `index`, ESC cancels.
    ConfirmDelete { index: usize },
    /// Text prompt; commits per purpose on ENTER, cancels on ESC.
    Prompt {
        purpose: PromptPurpose,
        index: usize,
    },
    /// Friendly failure text; any key dismisses.
    Error { message: &'static str },
    /// Chunked copy/move progress bar.
    Progress { done: usize, total: usize },
}

/// Context-menu actions offered for a selected entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MenuAction {
    Delete,
    Rename,
    Duplicate,
    MoveTo,
}

pub(crate) const MENU_ACTION_COUNT: usize = 4;

impl MenuAction {
    pub(crate) const ALL: [MenuAction; MENU_ACTION_COUNT] = [
        MenuAction::Delete,
        MenuAction::Rename,
        MenuAction::Duplicate,
        MenuAction::MoveTo,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            MenuAction::Delete => "DELETE",
            MenuAction::Rename => "RENAME",
            MenuAction::Duplicate => "DUPLICATE",
            MenuAction::MoveTo => "MOVE TO...",
        }
    }
}

/// Geometry of the context menu overlay; shared by renderer and the
/// pointer hit-test so clicks map onto exactly what was drawn.
pub(crate) const MENU_X: i32 = LIST_X as i32 + 8;
pub(crate) const MENU_Y: i32 = LIST_Y as i32 + 8;
pub(crate) const MENU_WIDTH: i32 = 160;
pub(crate) const MENU_HEADER_H: i32 = 16;

/// Maps a pointer position to a highlighted menu row (0..MENU_ACTION_COUNT),
/// or None when the click falls outside the menu body.
pub(crate) fn menu_hit(x: i32, y: i32) -> Option<usize> {
    if x < MENU_X || x >= MENU_X + MENU_WIDTH || y < MENU_Y + MENU_HEADER_H {
        return None;
    }
    let row = (y - MENU_Y - MENU_HEADER_H) / ROW_HEIGHT as i32;
    (0..MENU_ACTION_COUNT as i32)
        .contains(&row)
        .then(|| row as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_hit_maps_rows_and_rejects_outside() {
        let mid_x = MENU_X + MENU_WIDTH / 2;
        let row_y = |row: i32| MENU_Y + MENU_HEADER_H + row * ROW_HEIGHT as i32 + 4;
        assert_eq!(menu_hit(mid_x, row_y(0)), Some(0));
        assert_eq!(menu_hit(mid_x, row_y(3)), Some(3));
        assert_eq!(menu_hit(mid_x, row_y(4)), None, "below last action");
        assert_eq!(menu_hit(MENU_X - 1, row_y(0)), None, "left of box");
        assert_eq!(menu_hit(mid_x, MENU_Y - 1), None, "above box");
    }

    #[test]
    fn menu_actions_have_labels_for_every_row() {
        assert_eq!(MenuAction::ALL.len(), MENU_ACTION_COUNT);
        for action in MenuAction::ALL {
            assert!(!action.label().is_empty());
        }
    }
}
