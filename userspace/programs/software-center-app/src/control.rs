use serviceos_userspace_runtime as rt;
use rt::{AppControlTag, RawMessage};

use crate::actions::{apply_selected_package_action, sync_repositories, PackageAction};
use crate::lifecycle::{key_action_from_word, pointer_action_from_word};
use crate::render::render;
use crate::state::{
    clamp_view, compute_layout, ensure_selected_visible, scroll_down, scroll_up, selected_entry,
    visible_row_count, AppState, KEY_BACKSPACE, KEY_DELETE, KEY_DOWN, KEY_ENTER, KEY_PAGE_DOWN,
    KEY_PAGE_UP, KEY_R, KEY_UP, ROW_HEIGHT, SURFACE_BUFFER_SLOTS,
};

pub(crate) enum ControlFlow {
    Idle,
    Worked,
    Exit,
}

pub(crate) fn poll_control(
    control_handle: rt::Handle,
    surface_handle: rt::Handle,
    package_handle: rt::Handle,
    buffers: &mut [Option<rt::MappedMemory>; SURFACE_BUFFER_SLOTS],
    front_buffer_slot: &mut usize,
    state: &mut AppState,
) -> rt::Result<ControlFlow> {
    let mut changed = false;
    let mut did_work = false;
    loop {
        let mut message = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(control_handle, &mut message) {
            Ok(()) if message.tag == AppControlTag::FocusChanged as u32 && message.word_count > 0 => {
                state.focused = message.words[0] != 0;
                changed = true;
                did_work = true;
            }
            Ok(()) if message.tag == AppControlTag::Resize as u32 && message.word_count >= 2 => {
                state.width = message.words[0] as u32;
                state.height = message.words[1] as u32;
                clamp_view(state);
                changed = true;
                did_work = true;
            }
            Ok(()) if message.tag == AppControlTag::Pointer as u32 && message.word_count >= 5 => {
                let action = pointer_action_from_word(message.words[0]);
                let x = message.words[1] as i64 as i32;
                let y = message.words[2] as i64 as i32;
                let detail = message.words[4] as i64 as i32;
                did_work = true;
                match action {
                    Some(rt::AppPointerAction::Down) => {
                        changed |= handle_pointer_down(package_handle, state, x, y)?;
                    }
                    Some(rt::AppPointerAction::Scroll) => {
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
                if matches!(key_action_from_word(message.words[0]), Some(rt::AppKeyAction::Down)) {
                    changed |= handle_key_down(package_handle, state, message.words[1] as u32)?;
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
        *front_buffer_slot = (*front_buffer_slot + 1) % SURFACE_BUFFER_SLOTS;
        render(
            surface_handle,
            *front_buffer_slot as u32,
            buffers[*front_buffer_slot].as_mut().unwrap(),
            package_handle,
            state,
        )?;
        return Ok(ControlFlow::Worked);
    }
    if did_work {
        return Ok(ControlFlow::Worked);
    }
    Ok(ControlFlow::Idle)
}

fn handle_pointer_down(
    package_handle: rt::Handle,
    state: &mut AppState,
    x: i32,
    y: i32,
) -> rt::Result<bool> {
    let layout = compute_layout(state);
    if y >= layout.sync_y0 && y < layout.sync_y1 && x >= layout.sync_x0 && x < layout.sync_x1 {
        sync_repositories(package_handle, state);
        return Ok(true);
    }
    if y >= layout.install_y0 && y < layout.install_y1 && x >= layout.install_x0 && x < layout.install_x1 {
        if let Some(entry) = selected_entry(state) {
            apply_selected_package_action(package_handle, state, entry, PackageAction::InstallOrUpdate);
            return Ok(true);
        }
    }
    if y >= layout.remove_y0 && y < layout.remove_y1 && x >= layout.remove_x0 && x < layout.remove_x1 {
        if let Some(entry) = selected_entry(state) {
            apply_selected_package_action(package_handle, state, entry, PackageAction::Remove);
            return Ok(true);
        }
    }

    let visible_rows = layout.visible_rows();
    if x >= layout.left_x + 8 && x < layout.left_x + layout.left_w - 8 && y >= layout.list_rows_y {
        let row = ((y - layout.list_rows_y) / ROW_HEIGHT) as usize;
        let entry_index = state.scroll_offset + row;
        if row < visible_rows && entry_index < state.entry_count {
            state.selected_index = entry_index;
            return Ok(true);
        }
    }
    Ok(false)
}

fn handle_key_down(package_handle: rt::Handle, state: &mut AppState, key: u32) -> rt::Result<bool> {
    match key {
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
            let step = visible_row_count(state.height).max(1);
            state.selected_index = state.selected_index.saturating_sub(step);
            ensure_selected_visible(state);
            return Ok(true);
        }
        KEY_PAGE_DOWN => {
            let step = visible_row_count(state.height).max(1);
            if state.entry_count > 0 {
                state.selected_index = (state.selected_index + step).min(state.entry_count - 1);
                ensure_selected_visible(state);
                return Ok(true);
            }
        }
        KEY_ENTER => {
            if let Some(entry) = selected_entry(state) {
                apply_selected_package_action(package_handle, state, entry, PackageAction::InstallOrUpdate);
                return Ok(true);
            }
        }
        KEY_BACKSPACE | KEY_DELETE => {
            if let Some(entry) = selected_entry(state).filter(|entry| entry.installed) {
                apply_selected_package_action(package_handle, state, entry, PackageAction::Remove);
                return Ok(true);
            }
        }
        KEY_R => {
            sync_repositories(package_handle, state);
            return Ok(true);
        }
        _ => {}
    }
    Ok(false)
}
