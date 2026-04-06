mod hit_test;
mod keyboard;
mod overlays;
mod pointer;

use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;
use rt::{AppKeyAction, AppPointerAction, DesktopAppId, DesktopInputAction};

use crate::{
    palette_matches,
    render::{render_desktop, render_overlays_only, sync_cursor},
    windows::{
        app_slot_index, clamp_window_x, clamp_window_y, close_app, focus_app,
        focused_surface_id, maximize_app, minimize_app, move_app, move_focused_to_workspace,
        post_notification, switch_workspace, visible_on_workspace,
    },
    ContentCapture, DesktopState, DragState, HitTarget, OverlayMode, PaletteAction,
    ResizeEdges, WindowState, APP_COUNT, CLIPBOARD_HISTORY_LINES, KEY_1, KEY_2, KEY_3, KEY_4,
    KEY_5, KEY_BACKSPACE, KEY_DOWN, KEY_ENTER, KEY_ESC, KEY_F4, KEY_LEFT_ALT, KEY_N,
    KEY_RIGHT_ALT, KEY_SPACE, KEY_TAB, KEY_UP, KEY_V, MOD_ALT, MOD_CTRL, MOD_SHIFT,
    OVERLAY_RESULT_MAX, PANEL_MARGIN, RESIZE_GRIP_SIZE, TOPBAR_HEIGHT, WINDOW_MIN_HEIGHT,
    WINDOW_MIN_WIDTH,
};

pub(crate) fn handle_input(
    state: &mut DesktopState,
    action: DesktopInputAction,
    x: i32,
    y: i32,
    detail: i32,
) -> rt::Result<u32> {
    let overlay_before = state.overlay_mode;
    let overlay_selection_before = state.overlay_selection;
    let palette_query_len_before = state.palette_query_len;
    let active_workspace_before = state.active_workspace;
    let focused_app_before = state.focused_app;

    let result = match action {
        DesktopInputAction::PointerDown => {
            state.pointer_x = x;
            state.pointer_y = y;
            pointer::handle_pointer_down(state, x, y)
        }
        DesktopInputAction::PointerMove => {
            state.pointer_x = x;
            state.pointer_y = y;
            pointer::handle_pointer_move(state, x, y)
        }
        DesktopInputAction::PointerUp => {
            state.pointer_x = x;
            state.pointer_y = y;
            let surface_id = pointer::handle_pointer_up(state, x, y)?;
            state.drag_state = None;
            state.content_capture = None;
            Ok(surface_id)
        }
        DesktopInputAction::Click => {
            state.pointer_x = x;
            state.pointer_y = y;
            let _ = pointer::handle_pointer_down(state, x, y)?;
            state.drag_state = None;
            state.content_capture = None;
            Ok(focused_surface_id(state))
        }
        DesktopInputAction::PointerScroll => {
            state.pointer_x = x;
            state.pointer_y = y;
            pointer::handle_pointer_scroll(state, x, y, detail)
        }
        DesktopInputAction::KeyDown => {
            keyboard::handle_key_input(state, AppKeyAction::Down, x as u32, y as u32)
        }
        DesktopInputAction::KeyUp => {
            keyboard::handle_key_input(state, AppKeyAction::Up, x as u32, y as u32)
        }
        DesktopInputAction::TextInput => keyboard::handle_text_input(state, x as u32),
    }?;
    sync_cursor(state)?;

    let shell_changed = state.overlay_mode != overlay_before
        || state.overlay_selection != overlay_selection_before
        || state.palette_query_len != palette_query_len_before
        || state.active_workspace != active_workspace_before
        || state.focused_app != focused_app_before;
    let overlay_changed = state.overlay_mode != overlay_before
        || state.overlay_selection != overlay_selection_before
        || state.palette_query_len != palette_query_len_before;
    let shell_core_changed = state.active_workspace != active_workspace_before
        || state.focused_app != focused_app_before;

    match action {
        DesktopInputAction::PointerMove | DesktopInputAction::PointerScroll => {}
        DesktopInputAction::KeyDown | DesktopInputAction::KeyUp | DesktopInputAction::TextInput => {
            if shell_changed {
                if overlay_changed && !shell_core_changed {
                    render_overlays_only(state)?;
                } else {
                    render_desktop(state)?;
                }
            }
        }
        _ => {
            if overlay_changed && !shell_core_changed {
                render_overlays_only(state)?;
            } else {
                render_desktop(state)?;
            }
        }
    }
    Ok(result)
}

pub(crate) fn focus_next_app(state: &mut DesktopState) -> rt::Result<u32> {
    overlays::focus_recent_app(state, 1)
}

pub(crate) fn focus_previous_app(state: &mut DesktopState) -> rt::Result<u32> {
    overlays::focus_recent_app(state, APP_COUNT - 1)
}

pub(crate) fn focus_next_visible_without_cycle(state: &mut DesktopState) -> rt::Result<u32> {
    let mut best: Option<(u32, DesktopAppId)> = None;
    for slot in state.apps.iter().copied() {
        if !slot.running || !slot.window.visible() || slot.workspace_id != state.active_workspace {
            continue;
        }
        match best {
            Some((z_order, _)) if z_order >= slot.window.z_order => {}
            _ => best = Some((slot.window.z_order, slot.app_id)),
        }
    }
    match best {
        Some((_, app_id)) => focus_app(state, app_id),
        None => Ok(0),
    }
}
