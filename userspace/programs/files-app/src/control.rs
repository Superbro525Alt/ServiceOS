use rt::{AppControlTag, AppKeyAction, AppPointerAction, DesktopAppId, RawMessage};
use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;

use crate::assoc;
use crate::bridge::send_content_intent;
use crate::navigation::{
    clamp_view, ensure_selected_visible, entry_name_bytes, navigate_parent, open_path_in_explorer,
    open_selected, reload_directory, reload_search, reopen_directory, scroll_down, scroll_up,
    visible_row_count,
};
use crate::ops::{self, CopyProgress, OpError};
use crate::persist;
use crate::render::render;
use crate::state::{
    DRAG_THRESHOLD_PX, Dialog, EntryKind, ExplorerState, KEY_BACKSPACE, KEY_D, KEY_DELETE,
    KEY_DOWN, KEY_ENTER, KEY_ESC, KEY_F2, KEY_LEFT, KEY_N, KEY_O, KEY_PAGE_DOWN, KEY_PAGE_UP,
    KEY_R, KEY_RIGHT, KEY_UP, MOD_CTRL, MOD_SHIFT, MenuAction, Press, SURFACE_BUFFER_SLOTS,
    ViewMode, menu_hit,
};

/// Candidate slot ceiling for open-with cycling.
const PICK_SLOTS: usize = 6;
/// Render cadence during chunked copies (one present per N chunks).
const PROGRESS_RENDER_EVERY: usize = 4;

/// Render plumbing threaded through long-running ops so progress can be
/// presented mid-operation.
struct UiOut<'a> {
    buffers: &'a mut ui::SurfaceBuffers<SURFACE_BUFFER_SLOTS>,
    presenter: &'a mut ui::FirstPresentSurface,
}

impl<'a> UiOut<'a> {
    fn render(&mut self, state: &ExplorerState) -> rt::Result<()> {
        let (slot, buffer) = self.buffers.advance();
        render(self.presenter, slot, buffer, state)
    }
}

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
                        if state.menu.is_some() {
                            changed |= handle_menu_pointer(state, x, y);
                        } else if let Some(Dialog::Error { .. }) = state.dialog {
                            // Click dismisses the failure note.
                            state.dialog = None;
                            changed = true;
                        } else {
                            changed |= handle_pointer_down(state, storage_handle, x, y)?;
                        }
                    }
                    Some(AppPointerAction::Move) => {
                        changed |= handle_pointer_move(state, x, y);
                    }
                    Some(AppPointerAction::Up) => {
                        changed |= handle_pointer_up(state, storage_handle)?;
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
                    let mut ui_out = UiOut { buffers, presenter };
                    changed |= handle_key_down(
                        state,
                        storage_handle,
                        desktop_handle,
                        message.words[1] as u32,
                        message.words.get(2).copied().unwrap_or(0) as u32,
                        Some(&mut ui_out),
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

    let pressed_entry = state.entries[index];
    if pressed_entry.kind == EntryKind::Parent {
        // The `..` row keeps its instant-navigate shortcut.
        state.selected_index = index;
        ensure_selected_visible(state);
        if open_selected(state, storage_handle).is_err() {
            state.load_failed = true;
        }
        return Ok(true);
    }

    // Files and directories arm a press: quick release clicks, pointer
    // travel on files becomes a drag, and a second click on the same row
    // opens its context menu.
    state.selected_index = index;
    ensure_selected_visible(state);
    state.press = Some(Press { index, x, y });
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

/// Pointer-up resolves a press into a click: navigation for directories,
/// selection for files, and a context menu when the click lands on the
/// row that was already awaiting a second click.
fn handle_pointer_up(state: &mut ExplorerState, storage_handle: rt::Handle) -> rt::Result<bool> {
    let Some(press) = state.press.take() else {
        return Ok(false);
    };
    if state.dragging {
        state.dragging = false;
        state.await_context = None;
        return Ok(false);
    }
    if press.index >= state.entry_count {
        state.await_context = None;
        return Ok(false);
    }

    let kind = state.entries[press.index].kind;
    let second_click = state.await_context == Some(press.index);
    state.await_context = None;
    if second_click && kind != EntryKind::Parent {
        state.menu = Some((press.index, 0));
        return Ok(true);
    }

    match kind {
        EntryKind::Parent => Ok(false),
        EntryKind::Directory => {
            state.await_context = Some(press.index);
            if open_selected(state, storage_handle).is_err() {
                state.load_failed = true;
            }
            Ok(true)
        }
        EntryKind::File => {
            state.await_context = Some(press.index);
            Ok(true)
        }
    }
}

fn handle_key_down(
    state: &mut ExplorerState,
    storage_handle: rt::Handle,
    desktop_handle: rt::Handle,
    key_code: u32,
    modifiers: u32,
    mut progress_ui: Option<&mut UiOut>,
) -> rt::Result<bool> {
    // Modal layers swallow input before list handling.
    if let Some(consumed) = handle_dialog_key(
        state,
        storage_handle,
        key_code,
        modifiers,
        progress_ui.as_deref_mut(),
    )? {
        return Ok(consumed);
    }
    if state.menu.is_some() {
        return Ok(handle_menu_key(
            state,
            storage_handle,
            key_code,
            progress_ui.as_deref_mut(),
        ));
    }
    if key_code == KEY_R && modifiers & MOD_CTRL != 0 {
        state.open_with_pick = None;
        state.view_mode = match state.view_mode {
            ViewMode::Recent => ViewMode::Directory,
            ViewMode::Directory | ViewMode::Search => ViewMode::Recent,
        };
        clamp_view(state);
        return Ok(true);
    }
    match state.view_mode {
        ViewMode::Recent => return handle_key_recent(state, storage_handle, key_code),
        ViewMode::Directory => {}
        ViewMode::Search => {
            return handle_key_search(state, storage_handle, desktop_handle, key_code, modifiers);
        }
    }

    if modifiers & MOD_CTRL == 0
        && let Some(character) = ops::scancode_to_char(key_code, modifiers)
    {
        if character.is_ascii_graphic() {
            state.search_query[0] = character;
            state.search_query_len = 1;
            state.view_mode = ViewMode::Search;
            if reload_search(state, storage_handle).is_err() {
                state.load_failed = true;
            }
            return Ok(true);
        }
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
        KEY_O if modifiers & MOD_CTRL != 0 => cycle_open_with_pick(state),
        KEY_D if modifiers & MOD_CTRL != 0 => commit_open_with_default(state),
        KEY_DELETE => {
            if let Some(index) = deletable_selection(state) {
                state.dialog = Some(Dialog::ConfirmDelete { index });
                state.await_context = None;
                return Ok(true);
            }
        }
        KEY_F2 => {
            if let Some(index) = deletable_selection(state) {
                begin_rename_prompt(state, index);
                return Ok(true);
            }
        }
        KEY_N if modifiers & MOD_CTRL != 0 => {
            let purpose = if modifiers & MOD_SHIFT != 0 {
                crate::state::PromptPurpose::NewFile
            } else {
                crate::state::PromptPurpose::NewFolder
            };
            begin_new_entry_prompt(state, purpose);
            return Ok(true);
        }
        KEY_ESC => {
            let had_pick = state.open_with_pick.take().is_some();
            let was_dragging = end_press(state);
            state.await_context = None;
            if had_pick || was_dragging {
                return Ok(true);
            }
        }
        _ => {}
    }
    Ok(false)
}

fn exit_search(state: &mut ExplorerState) {
    state.search_query.fill(0);
    state.search_query_len = 0;
    state.view_mode = ViewMode::Directory;
}

fn handle_key_search(
    state: &mut ExplorerState,
    storage_handle: rt::Handle,
    desktop_handle: rt::Handle,
    key_code: u32,
    modifiers: u32,
) -> rt::Result<bool> {
    match key_code {
        KEY_ESC => {
            exit_search(state);
            if reload_directory(state).is_err() {
                state.load_failed = true;
            }
        }
        KEY_BACKSPACE => {
            state.search_query_len = state.search_query_len.saturating_sub(1);
            state.search_query[state.search_query_len] = 0;
            if state.search_query_len == 0 {
                exit_search(state);
                if reload_directory(state).is_err() {
                    state.load_failed = true;
                }
            } else if reload_search(state, storage_handle).is_err() {
                state.load_failed = true;
            }
        }
        KEY_ENTER | KEY_RIGHT => {
            let Some(selected_kind) = state
                .entries
                .get(state.selected_index)
                .filter(|_| state.selected_index < state.entry_count)
                .map(|entry| entry.kind)
            else {
                return Ok(true);
            };
            let is_file = selected_kind == EntryKind::File;
            exit_search(state);
            if open_selected_routed(state, storage_handle, desktop_handle).is_err() {
                state.load_failed = true;
            }
            if is_file && reload_directory(state).is_err() {
                state.load_failed = true;
            }
        }
        KEY_UP => {
            if state.selected_index > 0 {
                state.selected_index -= 1;
                ensure_selected_visible(state);
            }
        }
        KEY_DOWN => {
            if state.selected_index + 1 < state.entry_count {
                state.selected_index += 1;
                ensure_selected_visible(state);
            }
        }
        _ => {
            if let Some(character) = ops::scancode_to_char(key_code, modifiers) {
                if (character.is_ascii_graphic() || character == b' ')
                    && state.search_query_len < state.search_query.len()
                {
                    state.search_query[state.search_query_len] = character;
                    state.search_query_len += 1;
                    if reload_search(state, storage_handle).is_err() {
                        state.load_failed = true;
                    }
                }
            }
        }
    }
    Ok(true)
}

/// Index of the selected entry when an op may target it (never the
/// `..` parent row, never an empty list).
fn deletable_selection(state: &ExplorerState) -> Option<usize> {
    if state.entry_count == 0 {
        return None;
    }
    let index = state.selected_index.min(state.entry_count - 1);
    (state.entries[index].kind != EntryKind::Parent).then_some(index)
}

/// Fills the prompt with the first unused "New Folder"/"new.txt" variant
/// derived from the current listing.
fn begin_new_entry_prompt(state: &mut ExplorerState, purpose: crate::state::PromptPurpose) {
    let base: &[u8] = match purpose {
        crate::state::PromptPurpose::NewFile => b"new.txt",
        _ => b"New Folder",
    };
    let taken = listing_taken_names(state);
    let variant = ops::next_available_name(base, taken).unwrap_or(0);
    state.prompt_len = ops::variant_name(base, variant, &mut state.prompt_input).unwrap_or(0);
    state.menu = None;
    state.await_context = None;
    state.dialog = Some(Dialog::Prompt {
        purpose,
        index: usize::MAX,
    });
}

// ---------------------------------------------------------------------
// Modal dialog + context menu handling
// ---------------------------------------------------------------------

/// Routes a key into the active modal. Returns `Some(consumed)` when a
/// dialog was open (consumed = whether the frame changed).
fn handle_dialog_key(
    state: &mut ExplorerState,
    storage_handle: rt::Handle,
    key_code: u32,
    modifiers: u32,
    progress_ui: Option<&mut UiOut>,
) -> rt::Result<Option<bool>> {
    let Some(dialog) = state.dialog else {
        return Ok(None);
    };
    match dialog {
        Dialog::Error { .. } => {
            // Any key dismisses the failure note.
            state.dialog = None;
            Ok(Some(true))
        }
        Dialog::Progress { .. } => Ok(Some(true)),
        Dialog::ConfirmDelete { index } => {
            match key_code {
                KEY_ENTER => {
                    run_delete(state, storage_handle, index);
                }
                KEY_ESC => state.dialog = None,
                _ => {}
            }
            Ok(Some(true))
        }
        Dialog::Prompt { purpose, index } => {
            match key_code {
                KEY_ESC => {
                    state.dialog = None;
                    state.prompt_len = 0;
                }
                KEY_ENTER => {
                    commit_prompt(state, storage_handle, purpose, index, progress_ui)?;
                }
                KEY_BACKSPACE => {
                    state.prompt_len = state.prompt_len.saturating_sub(1);
                    state.prompt_input[state.prompt_len] = 0;
                }
                _ => {
                    if let Some(character) = ops::scancode_to_char(key_code, modifiers) {
                        if let Some(len) =
                            ops::prompt_push(&mut state.prompt_input, state.prompt_len, character)
                        {
                            state.prompt_len = len;
                        }
                    }
                }
            }
            Ok(Some(true))
        }
    }
}

/// Deletes the confirmed entry and reloads; failures become friendly
/// error dialogs instead of crashes.
fn run_delete(state: &mut ExplorerState, storage_handle: rt::Handle, index: usize) {
    if index >= state.entry_count {
        state.dialog = None;
        return;
    }
    let entry = state.entries[index];
    let mut name = [0u8; ops::NAME_MAX];
    let raw = entry_name_bytes(&entry);
    let len = raw.len().min(name.len());
    name[..len].copy_from_slice(&raw[..len]);

    match ops::delete_entry(
        storage_handle,
        &state.current_path[..state.current_path_len],
        &name[..len],
    ) {
        Ok(()) => {
            state.dialog = None;
            if reopen_directory(state, storage_handle)
                .and_then(|_| reload_directory(state))
                .is_err()
            {
                state.load_failed = true;
            }
        }
        Err(error) => {
            state.dialog = Some(Dialog::Error {
                message: ops::friendly_error(error),
            });
        }
    }
}

/// Commits a prompt per its purpose; all failures surface as dialogs.
fn commit_prompt(
    state: &mut ExplorerState,
    storage_handle: rt::Handle,
    purpose: crate::state::PromptPurpose,
    index: usize,
    progress_ui: Option<&mut UiOut>,
) -> rt::Result<()> {
    let name_len = state.prompt_len;
    let mut name = [0u8; ops::NAME_MAX];
    name[..name_len].copy_from_slice(&state.prompt_input[..name_len]);
    let name = &name[..name_len];

    let parent_len = state.current_path_len;
    let mut parent = [0u8; crate::state::MAX_STORAGE_PATH];
    parent[..parent_len].copy_from_slice(&state.current_path[..parent_len]);
    let parent = &parent[..parent_len];

    if let Err(error) = ops::validate_entry_name(name) {
        state.dialog = Some(Dialog::Error {
            message: ops::friendly_error(error),
        });
        return Ok(());
    }

    match purpose {
        crate::state::PromptPurpose::NewFolder | crate::state::PromptPurpose::NewFile => {
            let kind = match purpose {
                crate::state::PromptPurpose::NewFolder => EntryKind::Directory,
                _ => EntryKind::File,
            };
            if listing_taken_names(state)(name) {
                state.dialog = Some(Dialog::Error {
                    message: ops::friendly_error(OpError::Exists),
                });
                return Ok(());
            }
            finish_simple_op(state, ops::create_entry(storage_handle, parent, name, kind));
        }
        crate::state::PromptPurpose::Rename => {
            if index >= state.entry_count {
                state.dialog = None;
                return Ok(());
            }
            let source = state.entries[index];
            let kind = source.kind;
            let mut src_path = [0u8; crate::state::MAX_STORAGE_PATH];
            let src_len = source.path_len.min(src_path.len());
            src_path[..src_len].copy_from_slice(&source.path[..src_len]);
            run_chunked_op(state, storage_handle, progress_ui, |progress| {
                ops::move_entry(storage_handle, kind, &src_path, parent, name, progress)
            });
        }
        crate::state::PromptPurpose::MoveTo => {
            if index >= state.entry_count {
                state.dialog = None;
                return Ok(());
            }
            let source = state.entries[index];
            let kind = source.kind;
            let mut src_path = [0u8; crate::state::MAX_STORAGE_PATH];
            let src_len = source.path_len.min(src_path.len());
            src_path[..src_len].copy_from_slice(&source.path[..src_len]);
            let segments = match ops::split_segments(&src_path) {
                Ok(segments) => segments,
                Err(error) => {
                    state.dialog = Some(Dialog::Error {
                        message: ops::friendly_error(error),
                    });
                    return Ok(());
                }
            };
            // Destination dir text from the prompt (root allowed as "").
            let mut dst_parent = [0u8; crate::state::MAX_STORAGE_PATH];
            dst_parent[..name_len].copy_from_slice(name);
            let dst_len = if name_len > 0
                && dst_parent[name_len - 1] != b'/'
                && name_len < dst_parent.len()
            {
                dst_parent[name_len] = b'/';
                name_len + 1
            } else {
                name_len
            };
            run_chunked_op(state, storage_handle, progress_ui, |progress| {
                ops::move_entry(
                    storage_handle,
                    kind,
                    &src_path,
                    &dst_parent[..dst_len],
                    &segments.name[..segments.name_len],
                    progress,
                )
            });
        }
    }
    Ok(())
}

/// Creates/reloads wrapper for non-copy operations.
fn finish_simple_op(state: &mut ExplorerState, result: Result<(), OpError>) {
    match result {
        Ok(()) => {
            state.dialog = None;
            state.prompt_len = 0;
        }
        Err(error) => {
            state.dialog = Some(Dialog::Error {
                message: ops::friendly_error(error),
            });
        }
    }
}

/// Runs a copy/move with live progress feedback (bounded render cadence)
/// and maps the outcome onto dialog state plus a directory reload.
fn run_chunked_op(
    state: &mut ExplorerState,
    storage_handle: rt::Handle,
    progress_ui: Option<&mut UiOut>,
    run: impl FnOnce(&mut dyn FnMut(CopyProgress)) -> Result<(), OpError>,
) {
    let mut ui_cell = progress_ui;
    let mut ticks_without_render = 0usize;
    {
        let mut progress = |tick: CopyProgress| {
            state.dialog = Some(Dialog::Progress {
                done: tick.chunks_done,
                total: tick.total_chunks,
            });
            ticks_without_render += 1;
            if ticks_without_render >= PROGRESS_RENDER_EVERY {
                ticks_without_render = 0;
                if let Some(out) = ui_cell.as_deref_mut() {
                    let _ = out.render(state);
                }
            }
        };
        let outcome = run(&mut progress);
        match outcome {
            Ok(()) => {
                state.dialog = None;
                state.menu = None;
                state.await_context = None;
                state.prompt_len = 0;
            }
            Err(error) => {
                state.dialog = Some(Dialog::Error {
                    message: ops::friendly_error(error),
                });
                return;
            }
        }
    }
    if reopen_directory(state, storage_handle).is_ok() {
        let _ = reload_directory(state);
    }
}

/// Keyboard navigation inside the open context menu.
fn handle_menu_key(
    state: &mut ExplorerState,
    storage_handle: rt::Handle,
    key_code: u32,
    progress_ui: Option<&mut UiOut>,
) -> bool {
    let Some((index, cursor)) = state.menu else {
        return false;
    };
    let count = crate::state::MENU_ACTION_COUNT;
    match key_code {
        KEY_UP => {
            state.menu = Some((index, (cursor + count - 1) % count));
            true
        }
        KEY_DOWN => {
            state.menu = Some((index, (cursor + 1) % count));
            true
        }
        KEY_ESC => {
            state.menu = None;
            true
        }
        KEY_ENTER => {
            let action = MenuAction::ALL[cursor.min(count - 1)];
            execute_menu_action(state, storage_handle, index, action, progress_ui);
            true
        }
        _ => true,
    }
}

/// Pointer equivalent of menu Enter: clicking a drawn action row.
fn handle_menu_pointer(state: &mut ExplorerState, x: i32, y: i32) -> bool {
    let Some((index, _)) = state.menu else {
        return false;
    };
    match menu_hit(x, y) {
        Some(row) => {
            let action = MenuAction::ALL[row.min(crate::state::MENU_ACTION_COUNT - 1)];
            execute_menu_action(state, rt::INVALID_HANDLE, index, action, None);
            true
        }
        None => {
            // Click outside the box closes it.
            state.menu = None;
            true
        }
    }
}

fn execute_menu_action(
    state: &mut ExplorerState,
    storage_handle: rt::Handle,
    index: usize,
    action: MenuAction,
    progress_ui: Option<&mut UiOut>,
) {
    match action {
        MenuAction::Delete => {
            state.menu = None;
            state.dialog = Some(Dialog::ConfirmDelete { index });
        }
        MenuAction::Rename => begin_rename_prompt(state, index),
        MenuAction::Duplicate => {
            state.menu = None;
            duplicate_entry(state, storage_handle, index, progress_ui);
        }
        MenuAction::MoveTo => begin_move_prompt(state, index),
    }
}

/// Opens the move prompt prefilled with the entry's parent directory so
/// renaming the directory segment relocates the entry.
fn begin_move_prompt(state: &mut ExplorerState, index: usize) {
    if index >= state.entry_count {
        return;
    }
    state.prompt_len = 0;
    state.menu = None;
    state.await_context = None;
    state.dialog = Some(Dialog::Prompt {
        purpose: crate::state::PromptPurpose::MoveTo,
        index,
    });
}

/// Copies an entry next to itself under the first unused name variant.
fn duplicate_entry(
    state: &mut ExplorerState,
    storage_handle: rt::Handle,
    index: usize,
    progress_ui: Option<&mut UiOut>,
) {
    if index >= state.entry_count {
        return;
    }
    let source = state.entries[index];
    let kind = source.kind;
    let mut src_path = [0u8; crate::state::MAX_STORAGE_PATH];
    let src_len = source.path_len.min(src_path.len());
    src_path[..src_len].copy_from_slice(&source.path[..src_len]);

    let base_name = entry_name_bytes(&source);
    let mut base = [0u8; ops::NAME_MAX];
    let base_len = base_name.len().min(base.len());
    base[..base_len].copy_from_slice(&base_name[..base_len]);
    let taken = listing_taken_names(state);
    let variant = match ops::next_available_name(&base[..base_len], taken) {
        Ok(variant) => variant,
        Err(error) => {
            state.dialog = Some(Dialog::Error {
                message: ops::friendly_error(error),
            });
            return;
        }
    };
    let mut target = [0u8; ops::NAME_MAX];
    let target_len = match ops::variant_name(&base[..base_len], variant, &mut target) {
        Ok(len) => len,
        Err(error) => {
            state.dialog = Some(Dialog::Error {
                message: ops::friendly_error(error),
            });
            return;
        }
    };

    let parent_len = state.current_path_len;
    let mut parent = [0u8; crate::state::MAX_STORAGE_PATH];
    parent[..parent_len].copy_from_slice(&state.current_path[..parent_len]);

    run_chunked_op(state, storage_handle, progress_ui, |progress| {
        if kind == EntryKind::Directory {
            ops::copy_tree(
                storage_handle,
                &src_path,
                &parent[..parent_len],
                &target[..target_len],
                0,
                progress,
            )
        } else {
            ops::copy_file(
                storage_handle,
                &src_path,
                &parent[..parent_len],
                &target[..target_len],
                progress,
            )
            .map(|_| ())
        }
    });
}

/// Closure over the current listing matching any existing entry name.
fn listing_taken_names(state: &ExplorerState) -> impl FnMut(&[u8]) -> bool + '_ {
    move |candidate: &[u8]| {
        (0..state.entry_count).any(|index| {
            let entry = state.entries[index];
            entry.kind != EntryKind::Parent && entry_name_bytes(&entry) == candidate
        })
    }
}

/// Opens the rename prompt prefilled with the entry's current name.
fn begin_rename_prompt(state: &mut ExplorerState, index: usize) {
    let name = entry_name_bytes(&state.entries[index]);
    let len = name.len().min(crate::ops::NAME_MAX);
    state.prompt_input[..len].copy_from_slice(&name[..len]);
    state.prompt_len = len;
    state.menu = None;
    state.await_context = None;
    state.dialog = Some(Dialog::Prompt {
        purpose: crate::state::PromptPurpose::Rename,
        index,
    });
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
            search_query: [0; crate::state::MAX_SEARCH_QUERY],
            search_query_len: 0,
            recent_sel: 0,
            press: None,
            dragging: false,
            open_with_pick: None,
            assoc: AssocTable::empty(),
            recent: crate::recent::RecentRing::empty(),
            persist_dir: rt::INVALID_HANDLE,
            dialog: None,
            prompt_input: [0u8; crate::ops::NAME_MAX],
            prompt_len: 0,
            menu: None,
            await_context: None,
        };
        state.entries[0].kind = EntryKind::File;
        state.entries[0].path_len = path.len();
        state.entries[0].path[..path.len()].copy_from_slice(path);
        state
    }

    fn search_state(paths: &[(&[u8], EntryKind)]) -> ExplorerState {
        let mut state = file_state(
            paths
                .first()
                .map(|(path, _)| *path)
                .unwrap_or(b"home/notes.txt"),
        );
        state.view_mode = ViewMode::Search;
        state.entry_count = 0;
        for (index, (path, kind)) in paths.iter().enumerate() {
            state.entries[index].kind = *kind;
            state.entries[index].path_len = path.len();
            state.entries[index].path[..path.len()].copy_from_slice(path);
            state.entry_count += 1;
        }
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
        assert!(state.press.is_some(), "directories arm presses for clicks");
        assert!(!handle_pointer_move(&mut state, 90, 90), "but never drag");
        assert!(!state.dragging);
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
                0,
                None
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
            handle_key_down(
                &mut state,
                rt::INVALID_HANDLE,
                rt::INVALID_HANDLE,
                KEY_R,
                MOD_CTRL,
                None,
            )
            .unwrap()
        );
        assert_eq!(state.view_mode, ViewMode::Recent);
        assert!(
            handle_key_down(
                &mut state,
                rt::INVALID_HANDLE,
                rt::INVALID_HANDLE,
                KEY_ESC,
                0,
                None
            )
            .unwrap()
        );
        assert_eq!(state.view_mode, ViewMode::Directory);
    }

    #[test]
    fn printable_typing_enters_bounded_search_and_escape_restores_directory() {
        let mut state = file_state(b"home/notes.txt");
        assert!(key(&mut state, 33, 0)); // KEY_F -> 'f'
        assert_eq!(state.view_mode, ViewMode::Search);
        assert_eq!(&state.search_query[..state.search_query_len], b"f");
        assert!(key(&mut state, 24, MOD_SHIFT)); // KEY_O -> 'O'
        assert_eq!(&state.search_query[..state.search_query_len], b"fO");
        assert!(key(&mut state, KEY_ESC, 0));
        assert_eq!(state.view_mode, ViewMode::Directory);
        assert_eq!(state.search_query_len, 0);
    }

    #[test]
    fn leading_space_does_not_enter_search_mode() {
        let mut state = file_state(b"home/notes.txt");
        assert!(!key(&mut state, 57, 0)); // KEY_SPACE
        assert_eq!(state.view_mode, ViewMode::Directory);
        assert_eq!(state.search_query_len, 0);
    }

    #[test]
    fn search_backspace_to_empty_restores_directory_listing_mode() {
        let mut state = file_state(b"home/notes.txt");
        state.view_mode = ViewMode::Search;
        state.search_query[0] = b'n';
        state.search_query_len = 1;
        assert!(key(&mut state, KEY_BACKSPACE, 0));
        assert_eq!(state.view_mode, ViewMode::Directory);
        assert_eq!(state.search_query_len, 0);
    }

    #[test]
    fn search_backspace_trims_query_without_exiting_until_empty() {
        let mut state = file_state(b"home/notes.txt");
        state.view_mode = ViewMode::Search;
        state.search_query[..2].copy_from_slice(b"no");
        state.search_query_len = 2;

        assert!(key(&mut state, KEY_BACKSPACE, 0));

        assert_eq!(state.view_mode, ViewMode::Search);
        assert_eq!(state.search_query_len, 1);
        assert_eq!(&state.search_query[..state.search_query_len], b"n");
        assert_eq!(state.search_query[1], 0);
    }

    #[test]
    fn search_selection_stays_within_result_bounds() {
        let mut state = search_state(&[
            (b"docs/".as_slice(), EntryKind::Directory),
            (b"docs/guide.txt".as_slice(), EntryKind::File),
            (b"docs/notes.txt".as_slice(), EntryKind::File),
        ]);

        assert!(key(&mut state, KEY_UP, 0));
        assert_eq!(state.selected_index, 0);

        assert!(key(&mut state, KEY_DOWN, 0));
        assert_eq!(state.selected_index, 1);
        assert!(key(&mut state, KEY_DOWN, 0));
        assert_eq!(state.selected_index, 2);
        assert!(key(&mut state, KEY_DOWN, 0));
        assert_eq!(state.selected_index, 2);
    }

    #[test]
    fn search_query_growth_stops_at_fixed_capacity() {
        let mut state = file_state(b"home/notes.txt");
        state.view_mode = ViewMode::Search;
        state.search_query.fill(b'x');
        state.search_query_len = state.search_query.len();

        assert!(key(&mut state, 33, 0));

        assert_eq!(state.search_query_len, crate::state::MAX_SEARCH_QUERY);
        assert!(state.search_query.iter().all(|byte| *byte == b'x'));
    }

    #[test]
    fn enter_on_search_directory_hit_exits_search_and_targets_directory() {
        let mut state = search_state(&[(b"docs/".as_slice(), EntryKind::Directory)]);
        state.search_query[0] = b'd';
        state.search_query_len = 1;

        assert!(key(&mut state, KEY_ENTER, 0));

        assert_eq!(state.view_mode, ViewMode::Directory);
        assert_eq!(state.search_query_len, 0);
        assert_eq!(&state.current_path[..state.current_path_len], b"docs/");
        assert!(state.load_failed);
    }

    #[test]
    fn enter_on_search_file_hit_exits_search_and_reuses_file_routing() {
        let mut state = search_state(&[(b"docs/readme".as_slice(), EntryKind::File)]);
        state.search_query[0] = b'r';
        state.search_query_len = 1;

        assert!(key(&mut state, KEY_ENTER, 0));

        assert_eq!(state.view_mode, ViewMode::Directory);
        assert_eq!(state.search_query_len, 0);
        assert_eq!(&state.current_path[..state.current_path_len], b"docs/");
        assert!(state.load_failed);
    }

    fn key(state: &mut ExplorerState, code: u32, modifiers: u32) -> bool {
        handle_key_down(
            state,
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            code,
            modifiers,
            None,
        )
        .unwrap()
    }

    #[test]
    fn delete_key_arms_confirm_esc_cancels_without_touching_storage() {
        let mut state = file_state(b"home/notes.txt");
        assert!(key(&mut state, KEY_DELETE, 0));
        assert_eq!(
            state.dialog,
            Some(crate::state::Dialog::ConfirmDelete { index: 0 })
        );
        assert!(key(&mut state, KEY_ESC, 0));
        assert_eq!(state.dialog, None);
    }

    #[test]
    fn confirm_delete_enter_surfaces_storage_failure_as_friendly_dialog() {
        let mut state = file_state(b"home/notes.txt");
        key(&mut state, KEY_DELETE, 0);
        assert!(key(&mut state, KEY_ENTER, 0));
        match state.dialog {
            Some(crate::state::Dialog::Error { message }) => {
                assert!(!message.is_empty());
            }
            other => panic!("expected error dialog, got {other:?}"),
        }
        // Any key dismisses the error.
        assert!(key(&mut state, KEY_ENTER, 0));
        assert_eq!(state.dialog, None);
    }

    #[test]
    fn ctrl_n_prefills_unique_folder_prompt_and_esc_cancels() {
        let mut state = file_state(b"home/docs/");
        state.entries[0].kind = EntryKind::Directory;
        assert!(key(&mut state, KEY_N, crate::state::MOD_CTRL));
        assert_eq!(
            state.dialog,
            Some(crate::state::Dialog::Prompt {
                purpose: crate::state::PromptPurpose::NewFolder,
                index: usize::MAX,
            })
        );
        assert_eq!(&state.prompt_input[..state.prompt_len], b"New Folder");
        assert!(key(&mut state, KEY_ESC, 0));
        assert_eq!(state.dialog, None);
    }

    #[test]
    fn f2_prefills_rename_prompt_with_selected_entry_name() {
        let mut state = file_state(b"home/notes.txt");
        assert!(key(&mut state, KEY_F2, 0));
        assert_eq!(
            state.dialog,
            Some(crate::state::Dialog::Prompt {
                purpose: crate::state::PromptPurpose::Rename,
                index: 0,
            })
        );
        assert_eq!(&state.prompt_input[..state.prompt_len], b"notes.txt");
    }

    #[test]
    fn typed_characters_append_to_active_prompt() {
        let mut state = file_state(b"home/notes.txt");
        key(&mut state, KEY_F2, 0);
        let before = state.prompt_len;
        assert!(key(&mut state, 33, 0)); // KEY_F -> 'f'
        assert_eq!(state.prompt_len, before + 1);
        assert_eq!(state.prompt_input[state.prompt_len - 1], b'f');
        // Backspace pops it again.
        assert!(key(&mut state, KEY_BACKSPACE, 0));
        assert_eq!(state.prompt_len, before);
    }

    #[test]
    fn second_click_on_same_row_opens_menu_and_first_click_does_not() {
        let mut state = file_state(b"home/notes.txt");
        assert!(handle_pointer_down(&mut state, rt::INVALID_HANDLE, 40, 70).unwrap());
        assert!(state.menu.is_none());
        assert!(handle_pointer_up(&mut state, rt::INVALID_HANDLE).unwrap());
        assert_eq!(state.await_context, Some(0));
        // Second press+up on the same row opens the context menu.
        assert!(handle_pointer_down(&mut state, rt::INVALID_HANDLE, 40, 70).unwrap());
        handle_pointer_up(&mut state, rt::INVALID_HANDLE).unwrap();
        assert_eq!(state.menu, Some((0, 0)));
        assert!(key(&mut state, KEY_ESC, 0));
        assert_eq!(state.menu, None);
    }

    #[test]
    fn menu_enter_on_delete_row_routes_into_confirm_dialog() {
        let mut state = file_state(b"home/notes.txt");
        state.menu = Some((0, 0)); // cursor on DELETE
        assert!(key(&mut state, KEY_ENTER, 0));
        assert_eq!(state.menu, None);
        assert_eq!(
            state.dialog,
            Some(crate::state::Dialog::ConfirmDelete { index: 0 })
        );
    }
}
