use rt::{AppControlTag, AppKeyAction, AppPointerAction, DesktopAppId, RawMessage};
use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;

use crate::assoc;
use crate::bridge::send_content_intent;
use crate::navigation::{
    clamp_view, ensure_selected_visible, navigate_parent, open_path_in_explorer, open_selected,
    reload_directory, reopen_directory, scroll_down, scroll_up, visible_row_count,
};
use crate::persist;
use crate::render::render;
use crate::state::{
    DRAG_THRESHOLD_PX, ExplorerState, KEY_BACKSPACE, KEY_D, KEY_DOWN, KEY_ENTER, KEY_ESC, KEY_LEFT,
    KEY_O, KEY_PAGE_DOWN, KEY_PAGE_UP, KEY_R, KEY_RIGHT, KEY_UP, MOD_SHIFT, Press,
    SURFACE_BUFFER_SLOTS, ViewMode,
};

/// Candidate slot ceiling for open-with cycling.
const PICK_SLOTS: usize = 6;

pub(crate) enum ControlFlow {
    Idle,
    Worked,
    Exit,
}

pub(crate) fn poll_control(
    control_handle: rt::Handle,
    buffers: &mut ui::SurfaceBuffers<SURFACE_BUFFER_SLOTS>,
    presenter: &mut ui::FirstPresentSurface,
    storage_handle: rt::Handle,
    desktop_handle: rt::Handle,
    state: &mut ExplorerState,
) -> rt::Result<ControlFlow> {
    let mut changed = false;
    let mut did_work = false;
    loop {
        let mut message = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(control_handle, &mut message) {
            Ok(())
                if message.tag == AppControlTag::FocusChanged as u32 && message.word_count > 0 =>
            {
                did_work = true;
                state.focused = message.words[0] != 0;
                changed = true;
            }
            Ok(()) if message.tag == AppControlTag::Resize as u32 && message.word_count >= 2 => {
                did_work = true;
                state.width = message.words[0] as u32;
                state.height = message.words[1] as u32;
                clamp_view(state);
                changed = true;
            }
            Ok(()) if message.tag == AppControlTag::Pointer as u32 && message.word_count >= 5 => {
                did_work = true;
                let action = ui::decode_app_pointer_action(message.words[0]);
                let x = message.words[1] as i64 as i32;
                let y = message.words[2] as i64 as i32;
                let detail = message.words[4] as i64 as i32;
                match action {
                    Some(AppPointerAction::Down) => {
                        changed |= handle_pointer_down(state, storage_handle, x, y)?;
                    }
                    Some(AppPointerAction::Move) => {
                        changed |= handle_pointer_move(state, x, y);
                    }
                    Some(AppPointerAction::Up) => {
                        changed |= end_press(state);
                    }
                    Some(AppPointerAction::Scroll) => {
                        if detail > 0 {
                            scroll_up(state, detail as usize);
                            changed = true;
                        } else if detail < 0 {
                            scroll_down(state, (-detail) as usize);
                            changed = true;
                        }
                    }
                    _ => {}
                }
            }
            Ok(()) if message.tag == AppControlTag::Key as u32 && message.word_count >= 2 => {
                did_work = true;
                if matches!(
                    ui::decode_app_key_action(message.words[0]),
                    Some(AppKeyAction::Down)
                ) {
                    changed |= handle_key_down(
                        state,
                        storage_handle,
                        desktop_handle,
                        message.words[1] as u32,
                        message.words.get(2).copied().unwrap_or(0) as u32,
                    )?;
                }
            }
            Ok(()) if message.tag == AppControlTag::OpenPath as u32 && message.word_count >= 1 => {
                did_work = true;
                let requested = message.words[0] as usize;
                let mut path = [0u8; crate::state::MAX_STORAGE_PATH];
                if rt::unpack_bytes(
                    &message.words[1..message.word_count as usize],
                    requested,
                    &mut path,
                )
                .is_ok()
                {
                    changed |=
                        open_path_in_explorer(state, storage_handle, &path[..requested]).is_ok();
                }
            }
            Ok(()) if message.tag == AppControlTag::Close as u32 => return Ok(ControlFlow::Exit),
            Ok(()) => {
                did_work = true;
            }
            Err(rt::Error::QueueEmpty) => break,
            Err(error) => return Err(error),
        }
    }

    if changed {
        let (slot, buffer) = buffers.advance();
        render(presenter, slot, buffer, state)?;
        return Ok(ControlFlow::Worked);
    }

    if did_work {
        Ok(ControlFlow::Worked)
    } else {
        Ok(ControlFlow::Idle)
    }
}

fn handle_pointer_down(
    state: &mut ExplorerState,
    storage_handle: rt::Handle,
    x: i32,
    y: i32,
) -> rt::Result<bool> {
    if x < crate::state::LIST_X as i32 || y < crate::state::LIST_Y as i32 {
        return Ok(false);
    }
    let visible_rows = visible_row_count(state);
    if visible_rows == 0 {
        return Ok(false);
    }
    let row = ((y as usize).saturating_sub(crate::state::LIST_Y)) / crate::state::ROW_HEIGHT;
    if row >= visible_rows {
        return Ok(false);
    }
    let index = state.scroll_offset + row;
    if index >= state.entry_count {
        return Ok(false);
    }

    state.selected_index = index;
    ensure_selected_visible(state);
    let pressed_entry = state.entries[index];
    if matches!(
        pressed_entry.kind,
        crate::state::EntryKind::Parent | crate::state::EntryKind::Directory
    ) {
        if open_selected(state, storage_handle).is_err() {
            state.load_failed = true;
        }
    } else {
        // Press on a file row may become a drag once the pointer travels.
        state.press = Some(Press { index, x, y });
    }
    Ok(true)
}

fn handle_pointer_move(state: &mut ExplorerState, x: i32, y: i32) -> bool {
    let Some(press) = state.press else {
        return false;
    };
    if state.dragging {
        return false;
    }
    let moved_x = (x - press.x).abs();
    let moved_y = (y - press.y).abs();
    if moved_x.max(moved_y) < DRAG_THRESHOLD_PX {
        return false;
    }
    if press.index >= state.entry_count {
        state.press = None;
        return false;
    }
    if !matches!(
        state.entries[press.index].kind,
        crate::state::EntryKind::File
    ) {
        state.press = None;
        return false;
    }
    state.dragging = true;
    true
}

fn end_press(state: &mut ExplorerState) -> bool {
    let was_dragging = state.press.take().is_some() && state.dragging;
    if was_dragging {
        state.dragging = false;
    }
    was_dragging
}

fn handle_key_down(
    state: &mut ExplorerState,
    storage_handle: rt::Handle,
    desktop_handle: rt::Handle,
    key_code: u32,
    modifiers: u32,
) -> rt::Result<bool> {
    if key_code == KEY_R {
        state.open_with_pick = None;
        state.view_mode = match state.view_mode {
            ViewMode::Directory => ViewMode::Recent,
            ViewMode::Recent => ViewMode::Directory,
        };
        clamp_view(state);
        return Ok(true);
    }
    match state.view_mode {
        ViewMode::Recent => return handle_key_recent(state, storage_handle, key_code),
        ViewMode::Directory => {}
    }

    let visible_rows = visible_row_count(state).max(1);
    match key_code {
        KEY_UP => {
            if state.selected_index > 0 {
                state.selected_index -= 1;
                ensure_selected_visible(state);
                return Ok(true);
            }
        }
        KEY_DOWN => {
            if state.selected_index + 1 < state.entry_count {
                state.selected_index += 1;
                ensure_selected_visible(state);
                return Ok(true);
            }
        }
        KEY_PAGE_UP => {
            let amount = visible_rows.saturating_sub(1).max(1);
            state.selected_index = state.selected_index.saturating_sub(amount);
            ensure_selected_visible(state);
            return Ok(true);
        }
        KEY_PAGE_DOWN => {
            let amount = visible_rows.saturating_sub(1).max(1);
            state.selected_index =
                (state.selected_index + amount).min(state.entry_count.saturating_sub(1));
            ensure_selected_visible(state);
            return Ok(true);
        }
        KEY_ENTER | KEY_RIGHT => {
            if open_selected_routed(state, storage_handle, desktop_handle).is_err() {
                state.load_failed = true;
            }
            return Ok(true);
        }
        KEY_LEFT | KEY_BACKSPACE => {
            if modifiers & MOD_SHIFT != 0 {
                state.scroll_offset = 0;
                state.selected_index = 0;
                return Ok(true);
            }
            if state.current_path_len != 0 {
                navigate_parent(state);
                let result =
                    reopen_directory(state, storage_handle).and_then(|_| reload_directory(state));
                if result.is_err() {
                    state.load_failed = true;
                }
                return Ok(true);
            }
        }
        KEY_O => cycle_open_with_pick(state),
        KEY_D => commit_open_with_default(state),
        KEY_ESC => {
            let had_pick = state.open_with_pick.take().is_some();
            let was_dragging = end_press(state);
            if had_pick || was_dragging {
                return Ok(true);
            }
        }
        _ => {}
    }
    Ok(false)
}

fn handle_key_recent(
    state: &mut ExplorerState,
    storage_handle: rt::Handle,
    key_code: u32,
) -> rt::Result<bool> {
    match key_code {
        KEY_UP => {
            state.recent_sel = state.recent_sel.saturating_sub(1);
            Ok(true)
        }
        KEY_DOWN => {
            if state.recent_sel + 1 < state.recent.len() {
                state.recent_sel += 1;
            }
            Ok(true)
        }
        KEY_ENTER | KEY_RIGHT => {
            let Some(path) = state.recent.get(state.recent_sel) else {
                return Ok(false);
            };
            let mut owned = [0u8; crate::state::MAX_STORAGE_PATH];
            owned[..path.len()].copy_from_slice(path);
            let len = path.len();
            if open_path_in_explorer(state, storage_handle, &owned[..len]).is_err() {
                state.load_failed = true;
                return Ok(true);
            }
            state.view_mode = ViewMode::Directory;
            clamp_view(state);
            Ok(true)
        }
        KEY_ESC | KEY_BACKSPACE | KEY_LEFT => {
            state.view_mode = ViewMode::Directory;
            clamp_view(state);
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn selected_file_ext(state: &ExplorerState) -> Option<(usize, [u8; 16])> {
    let selected = state.entries.get(state.selected_index)?;
    if !matches!(selected.kind, crate::state::EntryKind::File) {
        return None;
    }
    let mut ext = [0u8; 16];
    let raw = assoc::extension_of(&selected.path[..selected.path_len]);
    if raw.len() > ext.len() {
        return None;
    }
    ext[..raw.len()].copy_from_slice(raw);
    Some((raw.len(), ext))
}

pub(crate) fn current_candidates(
    state: &ExplorerState,
) -> ([Option<DesktopAppId>; PICK_SLOTS], usize) {
    let mut candidates = [None; PICK_SLOTS];
    let count = match selected_file_ext(state) {
        Some((len, ext)) => state.assoc.candidates(&ext[..len], &mut candidates),
        // No extension: normalization rejects the bare dot, so only the
        // fixed fallback order remains.
        None => state
            .assoc
            .candidates(b".", &mut candidates)
            .min(assoc::OPEN_CANDIDATE_APPS.len()),
    };
    (candidates, count)
}

fn picked_app(state: &ExplorerState) -> Option<DesktopAppId> {
    let (candidates, count) = current_candidates(state);
    let pick = state.open_with_pick?;
    candidates
        .get(pick)
        .copied()
        .flatten()
        .filter(|_| pick < count)
}

fn cycle_open_with_pick(state: &mut ExplorerState) {
    if !matches!(selected_file_ext(state), Some(_)) {
        state.open_with_pick = None;
        return;
    }
    let (_, count) = current_candidates(state);
    if count == 0 {
        state.open_with_pick = None;
        return;
    }
    state.open_with_pick = Some(match state.open_with_pick {
        Some(pick) if pick + 1 < count => pick + 1,
        Some(_) => {
            state.open_with_pick = None;
            return;
        }
        None => 0,
    });
}

/// Commits the current open-with pick as the stored default for the selected
/// extension (persisted); without a pick it clears any override.
fn commit_open_with_default(state: &mut ExplorerState) {
    let Some((ext_len, ext)) = selected_file_ext(state) else {
        return;
    };
    let ext_slice = &ext[..ext_len];
    let applied = match picked_app(state) {
        Some(app) => state.assoc.set_default(ext_slice, app),
        None => state.assoc.remove(ext_slice),
    };
    state.open_with_pick = None;
    if applied {
        persist::save_associations(state.persist_dir, &state.assoc);
    }
}

/// Enter on a selection: directories navigate, files route through the
/// association policy (explicit pick -> stored default -> Files locator).
fn open_selected_routed(
    state: &mut ExplorerState,
    storage_handle: rt::Handle,
    desktop_handle: rt::Handle,
) -> rt::Result<()> {
    if state.entry_count == 0 {
        return Ok(());
    }
    let selected = state.entries[state.selected_index.min(state.entry_count - 1)];
    if !matches!(selected.kind, crate::state::EntryKind::File) {
        return open_selected(state, storage_handle);
    }

    let mut path = [0u8; crate::state::MAX_STORAGE_PATH];
    path[..selected.path_len].copy_from_slice(&selected.path[..selected.path_len]);
    let path = &path[..selected.path_len];

    let app = picked_app(state).unwrap_or_else(|| {
        let (ext_len, ext) = selected_file_ext(state).unwrap_or((0, [0u8; 16]));
        assoc::route_app(&ext[..ext_len], &state.assoc, None)
    });
    state.open_with_pick = None;

    let opened = match app {
        DesktopAppId::Files => open_path_in_explorer(state, storage_handle, path).is_ok(),
        _ => send_content_intent(desktop_handle, assoc::hint_digit(app).unwrap_or(b'0'), path)
            .is_ok(),
    };
    if opened {
        state.recent.record(path);
        persist::save_recent(state.persist_dir, &state.recent);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assoc::AssocTable;
    use crate::state::EntryKind;

    fn file_state(path: &[u8]) -> ExplorerState {
        let mut state = ExplorerState {
            width: 800,
            height: 600,
            focused: true,
            loading_initial_directory: false,
            current_directory_handle: rt::INVALID_HANDLE,
            current_path: [0; crate::state::MAX_STORAGE_PATH],
            current_path_len: 0,
            entries: [crate::state::ExplorerEntry::empty(); crate::state::MAX_ENTRIES],
            entry_count: 1,
            selected_index: 0,
            scroll_offset: 0,
            load_failed: false,
            view_mode: ViewMode::Directory,
            recent_sel: 0,
            press: None,
            dragging: false,
            open_with_pick: None,
            assoc: AssocTable::empty(),
            recent: crate::recent::RecentRing::empty(),
            persist_dir: rt::INVALID_HANDLE,
        };
        state.entries[0].kind = EntryKind::File;
        state.entries[0].path_len = path.len();
        state.entries[0].path[..path.len()].copy_from_slice(path);
        state
    }

    #[test]
    fn pointer_travel_past_threshold_starts_and_ends_drag_on_file_rows() {
        let mut state = file_state(b"home/notes.txt");
        assert!(handle_pointer_down(&mut state, rt::INVALID_HANDLE, 40, 70).unwrap());
        assert!(state.press.is_some());
        // Sub-threshold jiggle keeps the gesture pending.
        assert!(!handle_pointer_move(&mut state, 44, 72));
        assert!(!state.dragging);
        // Crossing the threshold arms the drag.
        assert!(handle_pointer_move(&mut state, 52, 80));
        assert!(state.dragging);
        // Pointer-up ends the drag without treating it as a click.
        assert!(end_press(&mut state));
        assert!(!state.dragging);
        assert!(state.press.is_none());
        assert!(!end_press(&mut state));
    }

    #[test]
    fn small_jiggle_then_up_never_arms_drag() {
        let mut state = file_state(b"home/notes.txt");
        assert!(handle_pointer_down(&mut state, rt::INVALID_HANDLE, 40, 70).unwrap());
        assert!(!handle_pointer_move(&mut state, 43, 71));
        assert!(!end_press(&mut state));
        assert!(!state.dragging);
    }

    #[test]
    fn drag_gesture_only_applies_to_files() {
        let mut state = file_state(b"home/docs/");
        state.entries[0].kind = EntryKind::Directory;
        assert!(handle_pointer_down(&mut state, rt::INVALID_HANDLE, 40, 70).unwrap());
        assert!(state.press.is_none(), "directory rows are not draggable");
        assert!(!handle_pointer_move(&mut state, 90, 90));
    }

    #[test]
    fn esc_cancels_pick_before_other_handling() {
        let mut state = file_state(b"home/notes.txt");
        state.open_with_pick = Some(0);
        assert!(
            handle_key_down(
                &mut state,
                rt::INVALID_HANDLE,
                rt::INVALID_HANDLE,
                KEY_ESC,
                0
            )
            .unwrap()
        );
        assert!(state.open_with_pick.is_none());
    }

    #[test]
    fn open_with_cycle_advances_wraps_and_requires_file_selection() {
        let mut state = file_state(b"home/run.log");
        cycle_open_with_pick(&mut state);
        assert_eq!(state.open_with_pick, Some(0));
        cycle_open_with_pick(&mut state);
        assert_eq!(state.open_with_pick, Some(1));
        while state.open_with_pick.is_some() {
            cycle_open_with_pick(&mut state);
        }
        assert!(state.open_with_pick.is_none());
        // Directory selection disables cycling entirely.
        let mut dirs = file_state(b"home/docs/");
        dirs.entries[0].kind = EntryKind::Directory;
        cycle_open_with_pick(&mut dirs);
        assert!(dirs.open_with_pick.is_none());
    }

    #[test]
    fn committing_pick_writes_default_and_clears_without_pick_removes_it() {
        let mut state = file_state(b"home/run.log");
        state.open_with_pick = Some(0);
        commit_open_with_default(&mut state);
        assert!(state.open_with_pick.is_none());
        assert_eq!(
            state.assoc.default_for(b"log"),
            Some(DesktopAppId::Files),
            "first candidate becomes the persisted default"
        );
        // A second commit with no pick clears the override again.
        commit_open_with_default(&mut state);
        assert_eq!(state.assoc.default_for(b"log"), None);
    }

    #[test]
    fn recent_toggle_and_escape_return_to_directory_view() {
        let mut state = file_state(b"home/notes.txt");
        assert!(
            handle_key_down(&mut state, rt::INVALID_HANDLE, rt::INVALID_HANDLE, KEY_R, 0).unwrap()
        );
        assert_eq!(state.view_mode, ViewMode::Recent);
        assert!(
            handle_key_down(
                &mut state,
                rt::INVALID_HANDLE,
                rt::INVALID_HANDLE,
                KEY_ESC,
                0
            )
            .unwrap()
        );
        assert_eq!(state.view_mode, ViewMode::Directory);
    }
}
