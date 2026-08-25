use rt::{AppControlTag, AppKeyAction, AppPointerAction, RawMessage};
use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;

use crate::navigation::{
    clamp_view, ensure_selected_visible, navigate_parent, open_path_in_explorer, open_selected,
    reload_directory, reopen_directory, scroll_down, scroll_up, visible_row_count,
};
use crate::render::render;
use crate::state::{
    ExplorerState, KEY_BACKSPACE, KEY_DOWN, KEY_ENTER, KEY_LEFT, KEY_PAGE_DOWN, KEY_PAGE_UP,
    KEY_RIGHT, KEY_UP, MOD_SHIFT, SURFACE_BUFFER_SLOTS,
};

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
    if matches!(
        state.entries[index].kind,
        crate::state::EntryKind::Parent | crate::state::EntryKind::Directory
    ) && open_selected(state, storage_handle).is_err()
    {
        state.load_failed = true;
    }
    Ok(true)
}

fn handle_key_down(
    state: &mut ExplorerState,
    storage_handle: rt::Handle,
    key_code: u32,
    modifiers: u32,
) -> rt::Result<bool> {
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
            if open_selected(state, storage_handle).is_err() {
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
        _ => {}
    }
    Ok(false)
}
