use rt::{AppControlTag, RawMessage};
use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;

use crate::actions::{PackageAction, apply_selected_package_action, launch_guidance, set_statusf, sync_repositories};
use crate::catalog_meta::keycode_to_char;
use crate::repositories::{
    self, SourcesClick, SourcesKey, execute_add, refresh_sources, sync_selected,
};
use crate::render::render;
use crate::state::{
    AppState, KEY_BACKSPACE, KEY_DELETE, KEY_DOWN, KEY_ENTER, KEY_ESC, KEY_L, KEY_PAGE_DOWN,
    KEY_PAGE_UP, KEY_R, KEY_S, KEY_TAB, KEY_UP, ROW_HEIGHT, SURFACE_BUFFER_SLOTS, clamp_view,
    clear_query, compute_layout, cycle_category_filter, ensure_selected_visible, pop_query_char,
    push_query_char, scroll_down, scroll_up, selected_entry, visible_row_count,
};

pub(crate) enum ControlFlow {
    Idle,
    Worked,
    Exit,
}

pub(crate) fn poll_control(
    control_handle: rt::Handle,
    package_handle: rt::Handle,
    buffers: &mut ui::SurfaceBuffers<SURFACE_BUFFER_SLOTS>,
    presenter: &mut ui::FirstPresentSurface,
    state: &mut AppState,
) -> rt::Result<ControlFlow> {
    let mut changed = false;
    let mut did_work = false;
    loop {
        let mut message = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(control_handle, &mut message) {
            Ok(())
                if message.tag == AppControlTag::FocusChanged as u32 && message.word_count > 0 =>
            {
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
                let action = ui::decode_app_pointer_action(message.words[0]);
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
                if matches!(
                    ui::decode_app_key_action(message.words[0]),
                    Some(rt::AppKeyAction::Down)
                ) {
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
        let (slot, buffer) = buffers.advance();
        render(presenter, slot, buffer, package_handle, state)?;
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

    if state.sources.open {
        return Ok(handle_sources_pointer(package_handle, state, x, y, layout));
    }

    if y >= layout.install_y0
        && y < layout.install_y1
        && x >= layout.install_x0
        && x < layout.install_x1
    {
        if let Some(entry) = selected_entry(state) {
            apply_selected_package_action(
                package_handle,
                state,
                entry,
                PackageAction::InstallOrUpdate,
            );
            return Ok(true);
        }
    }
    if y >= layout.remove_y0
        && y < layout.remove_y1
        && x >= layout.remove_x0
        && x < layout.remove_x1
    {
        if let Some(entry) = selected_entry(state) {
            apply_selected_package_action(package_handle, state, entry, PackageAction::Remove);
            return Ok(true);
        }
    }

    let visible_rows = layout.visible_rows();
    if x >= layout.left_x + 8 && x < layout.left_x + layout.left_w - 8 && y >= layout.list_rows_y {
        let row = ((y - layout.list_rows_y) / ROW_HEIGHT) as usize;
        let view_position = state.scroll_offset + row;
        if row < visible_rows
            && view_position < state.view_count
            && state.view[view_position] < state.entry_count
        {
            state.selected_index = view_position;
            return Ok(true);
        }
    }
    Ok(false)
}

/// Sources-view pointer handling: routing decisions come from
/// `repositories::handle_pointer` (pure, host-tested); only the effects that
/// need the package channel run here.
fn handle_sources_pointer(
    package_handle: rt::Handle,
    state: &mut AppState,
    x: i32,
    y: i32,
    layout: crate::state::Layout,
) -> bool {
    match repositories::handle_pointer(&state.sources, layout, x, y) {
        SourcesClick::None => false,
        SourcesClick::SelectRepo(position) => {
            state.sources.selected = position;
            state.sources.ensure_visible(layout.visible_rows());
            true
        }
        SourcesClick::Field(field) => {
            state.sources.field = field;
            true
        }
        SourcesClick::CycleTrust => {
            state.sources.cycle_trust();
            true
        }
        SourcesClick::BeginReview => {
            if state.sources.begin_review() {
                true
            } else {
                set_statusf(
                    state,
                    format_args!("add needs name, url (and hex digest when pinned)"),
                );
                true
            }
        }
        SourcesClick::ConfirmAdd => {
            execute_add(package_handle, state);
            true
        }
        SourcesClick::CancelReview => {
            state.sources.cancel_review();
            true
        }
        SourcesClick::SyncThis => {
            sync_selected(package_handle, state);
            true
        }
    }
}

fn handle_sources_key(package_handle: rt::Handle, state: &mut AppState, key: u32) -> bool {
    match repositories::handle_key(&mut state.sources, key) {
        SourcesKey::None => {}
        SourcesKey::BeginReview => {
            if !state.sources.begin_review() {
                set_statusf(
                    state,
                    format_args!("add needs name, url (and hex digest when pinned)"),
                );
            }
            return true;
        }
        SourcesKey::ConfirmAdd => {
            execute_add(package_handle, state);
            return true;
        }
        SourcesKey::Back => {
            if state.sources.in_review() {
                state.sources.cancel_review();
            } else {
                state.sources.open = false;
                set_statusf(state, format_args!("sources closed"));
            }
            return true;
        }
    }
    let mut changed = false;
    if let Some(byte) = keycode_to_char(key) {
        if state.sources.push_field_char(byte) {
            changed = true;
        }
    }
    changed
}

fn handle_key_down(package_handle: rt::Handle, state: &mut AppState, key: u32) -> rt::Result<bool> {
    if state.sources.open {
        return Ok(handle_sources_key(package_handle, state, key));
    }
    match key {
        KEY_UP => {
            if state.selected_index > 0 {
                state.selected_index -= 1;
                ensure_selected_visible(state);
                return Ok(true);
            }
        }
        KEY_DOWN => {
            if state.selected_index + 1 < state.view_count {
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
            if state.view_count > 0 {
                state.selected_index = (state.selected_index + step).min(state.view_count - 1);
                ensure_selected_visible(state);
                return Ok(true);
            }
        }
        KEY_TAB => {
            cycle_category_filter(state);
            return Ok(true);
        }
        KEY_ENTER => {
            if let Some(entry) = selected_entry(state) {
                apply_selected_package_action(
                    package_handle,
                    state,
                    entry,
                    PackageAction::InstallOrUpdate,
                );
                return Ok(true);
            }
        }
        KEY_ESC => {
            clear_query(state);
            return Ok(true);
        }
        KEY_BACKSPACE | KEY_DELETE => {
            if pop_query_char(state) {
                return Ok(true);
            }
            if let Some(entry) = selected_entry(state).filter(|entry| entry.installed) {
                apply_selected_package_action(package_handle, state, entry, PackageAction::Remove);
                return Ok(true);
            }
        }
        KEY_R if state.query_len == 0 => {
            sync_repositories(package_handle, state);
            return Ok(true);
        }
        KEY_S if state.query_len == 0 => {
            state.sources.open = true;
            refresh_sources(package_handle, state);
            return Ok(true);
        }
        KEY_L if state.query_len == 0 => {
            if let Some(entry) = selected_entry(state) {
                launch_guidance(state, entry);
                return Ok(true);
            }
        }
        _ => {}
    }
    if let Some(byte) = keycode_to_char(key) {
        push_query_char(state, byte);
        return Ok(true);
    }
    Ok(false)
}
