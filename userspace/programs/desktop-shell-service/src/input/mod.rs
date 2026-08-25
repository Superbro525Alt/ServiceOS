mod hit_test;
mod keyboard;
mod overlays;
mod pointer;

use rt::{AppKeyAction, AppPointerAction, DesktopAppId, DesktopInputAction};
use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;

use crate::{
    APP_COUNT, CLIPBOARD_HISTORY_LINES, ContentCapture, DesktopState, DragState, HitTarget, KEY_1,
    KEY_2, KEY_3, KEY_4, KEY_5, KEY_A, KEY_BACKSPACE, KEY_DOWN, KEY_ENTER, KEY_EQUAL, KEY_ESC,
    KEY_F, KEY_F4, KEY_H, KEY_J, KEY_LEFT_ALT, KEY_M, KEY_MINUS, KEY_N, KEY_RIGHT_ALT, KEY_SPACE,
    KEY_TAB, KEY_UP, KEY_V, MOD_ALT, MOD_CTRL, MOD_SHIFT, OVERLAY_RESULT_MAX, OverlayMode,
    PaletteAction, RESIZE_GRIP_SIZE, ResizeEdges, WINDOW_MIN_HEIGHT, WINDOW_MIN_WIDTH, WindowState,
    access::{Corner, sync_zoom, zoom_unmap_point},
    palette_matches,
    render::{render_desktop, render_overlays_only, sync_cursor},
    windows::{
        app_slot_index, clamp_window_x, clamp_window_y, close_app, focus_app, focused_surface_id,
        maximize_app, minimize_app, move_app, move_focused_to_workspace, post_notification,
        restore_app, switch_workspace, visible_on_workspace,
    },
};

pub(crate) use hit_test::launcher_hover_app;

/// Screen -> logical canvas mapping while the magnifier transform is applied.
fn logical_pointer(state: &DesktopState, x: i32, y: i32) -> (i32, i32) {
    if state.zoom_applied && state.access.zoom_index != 0 {
        zoom_unmap_point(
            state.access.zoom_index,
            state.zoom_last_fx,
            state.zoom_last_fy,
            x,
            y,
        )
    } else {
        (x, y)
    }
}

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
    let switcher_selection_before = state.switcher_selection;
    let active_workspace_before = state.active_workspace;
    let focused_app_before = state.focused_app;

    let result = match action {
        DesktopInputAction::PointerDown => {
            state.pointer_x = x;
            state.pointer_y = y;
            let (x, y) = logical_pointer(state, x, y);
            pointer::handle_pointer_down(state, x, y)
        }
        DesktopInputAction::PointerMove => {
            state.pointer_x = x;
            state.pointer_y = y;
            let (x, y) = logical_pointer(state, x, y);
            pointer::handle_pointer_move(state, x, y)
        }
        DesktopInputAction::PointerUp => {
            state.pointer_x = x;
            state.pointer_y = y;
            let (x, y) = logical_pointer(state, x, y);
            let surface_id = pointer::handle_pointer_up(state, x, y)?;
            state.drag_state = None;
            state.content_capture = None;
            Ok(surface_id)
        }
        DesktopInputAction::Click => {
            state.pointer_x = x;
            state.pointer_y = y;
            let (x, y) = logical_pointer(state, x, y);
            let _ = pointer::handle_pointer_down(state, x, y)?;
            state.drag_state = None;
            state.content_capture = None;
            Ok(focused_surface_id(state))
        }
        DesktopInputAction::PointerScroll => {
            state.pointer_x = x;
            state.pointer_y = y;
            let (x, y) = logical_pointer(state, x, y);
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
    sync_zoom(state)?;

    let shell_changed = state.overlay_mode != overlay_before
        || state.overlay_selection != overlay_selection_before
        || state.palette_query_len != palette_query_len_before
        || state.switcher_selection != switcher_selection_before
        || state.active_workspace != active_workspace_before
        || state.focused_app != focused_app_before;
    let overlay_changed = state.overlay_mode != overlay_before
        || state.overlay_selection != overlay_selection_before
        || state.palette_query_len != palette_query_len_before
        || state.switcher_selection != switcher_selection_before;
    let shell_core_changed = state.active_workspace != active_workspace_before
        || state.focused_app != focused_app_before;
    let focus_only_changed = state.focused_app != focused_app_before
        && state.active_workspace == active_workspace_before
        && !overlay_changed;

    match action {
        DesktopInputAction::PointerMove | DesktopInputAction::PointerScroll => {}
        DesktopInputAction::KeyDown | DesktopInputAction::KeyUp | DesktopInputAction::TextInput => {
            if shell_changed {
                if overlay_changed && !shell_core_changed {
                    render_overlays_only(state)?;
                } else if focus_only_changed {
                    state.pending_focus_refresh.set();
                } else {
                    render_desktop(state)?;
                }
            }
        }
        _ => {
            if shell_changed {
                if overlay_changed && !shell_core_changed {
                    render_overlays_only(state)?;
                } else if focus_only_changed {
                    state.pending_focus_refresh.set();
                } else {
                    render_desktop(state)?;
                }
            }
        }
    }
    if state.pending_media_refresh.take() && state.overlay_mode == OverlayMode::Media {
        render_overlays_only(state)?;
    }
    Ok(result)
}

pub(crate) fn focus_next_app(state: &mut DesktopState) -> rt::Result<u32> {
    overlays::focus_recent_app(state, 1)
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

/// Hot-corner action dispatch: top-left task switcher, top-right notifications,
/// bottom-left launcher (command palette), bottom-right show-desktop toggle.
pub(crate) fn fire_corner_action(state: &mut DesktopState, corner: Corner) -> rt::Result<()> {
    match corner {
        Corner::TopLeft => {
            let model = crate::switcher::switcher_model(state);
            state.overlay_mode = OverlayMode::Switcher;
            state.switcher_selection = crate::switcher::open_selection(&model, state.focused_app);
            state.overlay_selection = 0;
            render_overlays_only(state)?;
        }
        Corner::TopRight => {
            state.overlay_mode = OverlayMode::Notifications;
            state.overlay_selection = 0;
            render_overlays_only(state)?;
        }
        Corner::BottomLeft => {
            state.overlay_mode = OverlayMode::CommandPalette;
            state.overlay_selection = 0;
            state.palette_query_len = 0;
            render_overlays_only(state)?;
        }
        Corner::BottomRight => toggle_show_desktop(state)?,
    }
    Ok(())
}

fn toggle_show_desktop(state: &mut DesktopState) -> rt::Result<()> {
    if state.show_desktop_active {
        let mask = state.show_desktop_restore_mask;
        for index in 0..APP_COUNT {
            if mask & (1 << index) != 0 {
                restore_app(state, state.apps[index].app_id)?;
            }
        }
        state.show_desktop_active = false;
        state.show_desktop_restore_mask = 0;
    } else {
        let mut mask = 0u8;
        for index in 0..APP_COUNT {
            let slot = &state.apps[index];
            if slot.running && slot.window.visible() && slot.workspace_id == state.active_workspace
            {
                mask |= 1 << index;
            }
        }
        for index in 0..APP_COUNT {
            if mask & (1 << index) != 0 {
                minimize_app(state, state.apps[index].app_id)?;
            }
        }
        state.show_desktop_restore_mask = mask;
        state.show_desktop_active = true;
        render_desktop(state)?;
    }
    Ok(())
}
