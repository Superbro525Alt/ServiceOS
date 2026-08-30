use super::*;

pub(super) fn handle_pointer_down(state: &mut DesktopState, x: i32, y: i32) -> rt::Result<u32> {
    if state.overlay_mode != OverlayMode::None {
        if let Some(surface_id) = overlay_click(state, x, y)? {
            return Ok(surface_id);
        }
        state.overlay_mode = OverlayMode::None;
        state.overlay_selection = 0;
        state.switcher_selection = 0;
        render_overlays_only(state)?;
        return Ok(focused_surface_id(state));
    }
    match hit_test::hit_test(state, x, y) {
        HitTarget::Background => {
            state.drag_state = None;
            state.content_capture = None;
            Ok(focused_surface_id(state))
        }
        HitTarget::Launcher(app_id) => crate::windows::schedule_launch_or_focus_app(state, app_id),
        HitTarget::LauncherDoc(row) => crate::launcher_docs::open_launcher_doc(state, row),
        HitTarget::WindowContent(app_id) => {
            state.drag_state = None;
            state.content_capture = Some(ContentCapture { app_id, button: 1 });
            let surface_id = if state.focused_app == Some(app_id) {
                focused_surface_id(state)
            } else {
                focus_app(state, app_id)?
            };
            let (local_x, local_y) = app_local_coords(state, app_id, x, y)?;
            dispatch_pointer_to_app(
                state,
                app_id,
                AppPointerAction::Down,
                local_x,
                local_y,
                1,
                0,
            )?;
            Ok(surface_id)
        }
        HitTarget::WindowMove {
            app_id,
            grab_offset_x,
            grab_offset_y,
        } => {
            state.content_capture = None;
            let surface_id = if state.focused_app == Some(app_id) {
                focused_surface_id(state)
            } else {
                focus_app(state, app_id)?
            };
            state.drag_snap_zone = crate::windows::SnapZone::None;
            state.drag_state = Some(DragState::Move {
                app_id,
                grab_offset_x,
                grab_offset_y,
            });
            Ok(surface_id)
        }
        HitTarget::WindowResize { app_id, edges } => {
            state.content_capture = None;
            let surface_id = if state.focused_app == Some(app_id) {
                focused_surface_id(state)
            } else {
                focus_app(state, app_id)?
            };
            let index = app_slot_index(&state.apps, app_id).ok_or(rt::Error::NotFound)?;
            state.drag_state = Some(DragState::Resize {
                app_id,
                edges,
                origin_pointer_x: x,
                origin_pointer_y: y,
                start_x: state.apps[index].window.x,
                start_y: state.apps[index].window.y,
                start_width: state.apps[index].window.width,
                start_height: state.apps[index].window.height,
            });
            Ok(surface_id)
        }
        HitTarget::WindowClose(app_id) => {
            state.content_capture = None;
            close_app(state, app_id)?;
            Ok(focused_surface_id(state))
        }
        HitTarget::WindowMaximize(app_id) => {
            state.content_capture = None;
            maximize_app(state, app_id)
        }
        HitTarget::WindowMinimize(app_id) => minimize_app(state, app_id),
    }
}

pub(super) fn handle_pointer_move(state: &mut DesktopState, x: i32, y: i32) -> rt::Result<u32> {
    match state.drag_state {
        Some(DragState::Move {
            app_id,
            grab_offset_x,
            grab_offset_y,
        }) => {
            let zone = crate::windows::snap_zone_at(
                x,
                y,
                state.chrome.output_width,
                state.chrome.output_height,
            );
            if zone != state.drag_snap_zone {
                state.drag_snap_zone = zone;
                let _ = crate::windows::update_snap_preview(state, zone);
            }
            move_app(state, app_id, x - grab_offset_x, y - grab_offset_y)
        }
        Some(DragState::Resize {
            app_id,
            edges,
            origin_pointer_x,
            origin_pointer_y,
            start_x,
            start_y,
            start_width,
            start_height,
        }) => resize_drag(
            state,
            app_id,
            edges,
            origin_pointer_x,
            origin_pointer_y,
            start_x,
            start_y,
            start_width,
            start_height,
            x,
            y,
        ),
        None => {
            if let Some(capture) = state.content_capture {
                let (local_x, local_y) = app_local_coords(state, capture.app_id, x, y)?;
                dispatch_pointer_to_app(
                    state,
                    capture.app_id,
                    AppPointerAction::Move,
                    local_x,
                    local_y,
                    capture.button,
                    0,
                )?;
            }
            if state.content_drag.is_some() {
                let _ = crate::windows::update_drag_ghost(state, x, y);
            }
            Ok(focused_surface_id(state))
        }
    }
}

pub(super) fn handle_pointer_up(state: &mut DesktopState, x: i32, y: i32) -> rt::Result<u32> {
    if let Some(capture) = state.content_capture {
        let (local_x, local_y) = app_local_coords(state, capture.app_id, x, y)?;
        dispatch_pointer_to_app(
            state,
            capture.app_id,
            AppPointerAction::Up,
            local_x,
            local_y,
            capture.button,
            0,
        )?;
    }
    complete_content_drop(state, x, y)?;
    if let Some(DragState::Move { app_id, .. }) = state.drag_state {
        let zone = state.drag_snap_zone;
        if zone != crate::windows::SnapZone::None {
            state.drag_snap_zone = crate::windows::SnapZone::None;
            let _ = crate::windows::hide_snap_preview(state);
            match zone {
                crate::windows::SnapZone::LeftHalf => {
                    crate::windows::snap_window_half(state, app_id, true)?;
                }
                crate::windows::SnapZone::RightHalf => {
                    crate::windows::snap_window_half(state, app_id, false)?;
                }
                crate::windows::SnapZone::MinimizeBottom => {
                    minimize_app(state, app_id)?;
                }
                crate::windows::SnapZone::None => {}
            }
            return Ok(focused_surface_id(state));
        }
    }
    if let Some(DragState::Resize { .. }) = state.drag_state {
        crate::windows::flush_pending_resize(state)?;
    }
    Ok(focused_surface_id(state))
}

/// Finishes an armed content drag (file dragged out of files-app): the drop
/// target is whatever sits under the pointer now. Launcher icons receive the
/// open-path intent, the bare desktop canvas reveals the file in files-app,
/// and dropping back over any window cancels.
fn complete_content_drop(state: &mut DesktopState, x: i32, y: i32) -> rt::Result<()> {
    let Some(drag) = state.content_drag.take() else {
        return Ok(());
    };
    let _ = crate::windows::hide_drag_ghost(state);
    let Ok(path) = core::str::from_utf8(&drag.path[..drag.path_len]) else {
        return Ok(());
    };
    match crate::windows::drop_decision(&hit_test::hit_test(state, x, y)) {
        crate::windows::DropDecision::Deliver(app_id) => {
            crate::windows::deliver_open_intent(state, app_id, path)?;
            if drag.count > 1 {
                let mut notice = [0u8; 19];
                notice.copy_from_slice(b"OPENED 1 OF ? FILES");
                notice[11] = b'0' + drag.count.min(9) as u8;
                crate::windows::post_notification(state, Some(app_id), false, false, &notice)?;
            }
            state.pending_shell_refresh.set();
        }
        crate::windows::DropDecision::Cancel => {
            state.pending_shell_refresh.set();
        }
    }
    Ok(())
}

pub(super) fn handle_pointer_scroll(
    state: &mut DesktopState,
    x: i32,
    y: i32,
    delta_y: i32,
) -> rt::Result<u32> {
    match hit_test::hit_test(state, x, y) {
        HitTarget::WindowContent(app_id) => {
            let surface_id = if state.focused_app == Some(app_id) {
                focused_surface_id(state)
            } else {
                focus_app(state, app_id)?
            };
            let (local_x, local_y) = app_local_coords(state, app_id, x, y)?;
            dispatch_pointer_to_app(
                state,
                app_id,
                AppPointerAction::Scroll,
                local_x,
                local_y,
                0,
                delta_y,
            )?;
            Ok(surface_id)
        }
        _ => Ok(focused_surface_id(state)),
    }
}

pub(crate) fn dispatch_pointer_to_app(
    state: &DesktopState,
    app_id: DesktopAppId,
    action: AppPointerAction,
    local_x: i32,
    local_y: i32,
    button: u32,
    detail: i32,
) -> rt::Result<()> {
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Err(rt::Error::NotFound);
    };
    let control = state.apps[index].window.control_handle;
    if control == rt::INVALID_HANDLE {
        return Err(rt::Error::NotFound);
    }
    rt::app_control_pointer(control, action, local_x, local_y, button, detail)
}

fn app_local_coords(
    state: &DesktopState,
    app_id: DesktopAppId,
    x: i32,
    y: i32,
) -> rt::Result<(i32, i32)> {
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Err(rt::Error::NotFound);
    };
    let window = state.apps[index].window;
    let max_x = (window.width.saturating_sub(1)) as i32;
    let max_y = (window.height.saturating_sub(1)) as i32;
    Ok((
        (x - window.x).clamp(0, max_x),
        (y - window.y).clamp(0, max_y),
    ))
}

#[allow(clippy::too_many_arguments)]
fn resize_drag(
    state: &mut DesktopState,
    app_id: DesktopAppId,
    edges: ResizeEdges,
    origin_pointer_x: i32,
    origin_pointer_y: i32,
    start_x: i32,
    start_y: i32,
    start_width: u32,
    start_height: u32,
    x: i32,
    y: i32,
) -> rt::Result<u32> {
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Err(rt::Error::NotFound);
    };
    let delta_x = x - origin_pointer_x;
    let delta_y = y - origin_pointer_y;

    let mut new_x = start_x;
    let mut new_y = start_y;
    let mut new_width = start_width as i32;
    let mut new_height = start_height as i32;

    if edges.contains(ResizeEdges::LEFT) {
        new_x = start_x + delta_x;
        new_width = start_width as i32 - delta_x;
    }
    if edges.contains(ResizeEdges::RIGHT) {
        new_width = start_width as i32 + delta_x;
    }
    if edges.contains(ResizeEdges::TOP) {
        new_y = start_y + delta_y;
        new_height = start_height as i32 - delta_y;
    }
    if edges.contains(ResizeEdges::BOTTOM) {
        new_height = start_height as i32 + delta_y;
    }

    if new_width < WINDOW_MIN_WIDTH as i32 {
        if edges.contains(ResizeEdges::LEFT) {
            new_x -= WINDOW_MIN_WIDTH as i32 - new_width;
        }
        new_width = WINDOW_MIN_WIDTH as i32;
    }
    if new_height < WINDOW_MIN_HEIGHT as i32 {
        if edges.contains(ResizeEdges::TOP) {
            new_y -= WINDOW_MIN_HEIGHT as i32 - new_height;
        }
        new_height = WINDOW_MIN_HEIGHT as i32;
    }

    state.apps[index].window.maximized = false;
    state.apps[index].window.x = clamp_window_x(state.chrome.output_width, new_width as u32, new_x);
    state.apps[index].window.y =
        clamp_window_y(state.chrome.output_height, new_height as u32, new_y);
    state.apps[index].window.width = new_width as u32;
    state.apps[index].window.height = new_height as u32;
    rt::surface_set_geometry_async(
        state.apps[index].window.surface_handle,
        state.apps[index].window.x,
        state.apps[index].window.y,
        state.apps[index].window.width,
        state.apps[index].window.height,
        state.apps[index].window.z_order,
    )?;
    if state.apps[index].window.control_handle != rt::INVALID_HANDLE {
        state.pending_resize = Some(crate::PendingResize {
            app_id,
            width: state.apps[index].window.width,
            height: state.apps[index].window.height,
        });
    }
    Ok(state.apps[index].window.surface_id)
}

/// Quick-action strip inside the notification panel: local-y band and halves.
const NOTIF_QA_LOCAL_Y: i32 = 148;
const NOTIF_QA_HEIGHT: i32 = 16;

fn overlay_click(state: &mut DesktopState, x: i32, y: i32) -> rt::Result<Option<u32>> {
    let Some((ox, oy, ow, oh)) = crate::chrome::overlay_rect(&state.chrome, state.overlay_mode)
    else {
        return Ok(None);
    };
    if x < ox || y < oy || x >= ox + ow || y >= oy + oh {
        return Ok(None);
    }
    let local_x = x - ox;
    let local_y = y - oy;
    match state.overlay_mode {
        OverlayMode::ClipboardHistory => clipboard_click(state, local_y),
        OverlayMode::Notifications => notification_click(state, local_x, local_y),
        OverlayMode::CommandPalette => palette_click(state, local_y),
        OverlayMode::WorkspaceOverview => overview_tile_click(state, local_x, local_y),
        OverlayMode::Approval => approval_click(state, local_x, local_y),
        _ => Ok(None),
    }
}

/// Approval card quick-action strip: approve on the left half, deny on the
/// right — the pointer equivalent of the A/D keys.
fn approval_click(state: &mut DesktopState, local_x: i32, local_y: i32) -> rt::Result<Option<u32>> {
    if local_y >= crate::APPROVAL_QA_LOCAL_Y
        && local_y < crate::APPROVAL_QA_LOCAL_Y + crate::APPROVAL_QA_HEIGHT
    {
        let policy = if local_x < crate::APPROVAL_WIDTH as i32 / 2 {
            rt::PermissionPolicyState::Allowed
        } else {
            rt::PermissionPolicyState::Blocked
        };
        crate::approvals::decide_first_card(state, policy)?;
        return Ok(Some(focused_surface_id(state)));
    }
    Ok(None)
}

fn overview_tile_click(
    state: &mut DesktopState,
    local_x: i32,
    local_y: i32,
) -> rt::Result<Option<u32>> {
    let Some(index) = crate::windows::overview_tile_at(local_x, local_y) else {
        return Ok(None);
    };
    state.overlay_mode = OverlayMode::None;
    state.overlay_selection = 0;
    switch_workspace(state, index as u32 + 1).map(Some)
}

fn clipboard_click(state: &mut DesktopState, local_y: i32) -> rt::Result<Option<u32>> {
    let Some(row) = crate::chrome::overlay_row_at(local_y, CLIPBOARD_HISTORY_LINES) else {
        return Ok(None);
    };
    if state.clipboard_service_handle == rt::INVALID_HANDLE {
        return Ok(None);
    }
    if rt::clipboard_history_entry(state.clipboard_service_handle, row as u32).is_err() {
        return Ok(Some(focused_surface_id(state)));
    }
    overlays::paste_clipboard_selection(state, row).map(Some)
}

fn notification_click(
    state: &mut DesktopState,
    local_x: i32,
    local_y: i32,
) -> rt::Result<Option<u32>> {
    if local_y >= NOTIF_QA_LOCAL_Y && local_y < NOTIF_QA_LOCAL_Y + NOTIF_QA_HEIGHT {
        if local_x < crate::HISTORY_WIDTH as i32 / 2 {
            overlays::dismiss_all_notifications_now(state)?;
        } else if state.notification_history_len != 0 {
            return overlays::focus_notification_source(state).map(Some);
        }
        return Ok(Some(focused_surface_id(state)));
    }
    let Some(row) = crate::chrome::overlay_row_at(local_y, crate::NOTIFICATION_HISTORY_MAX) else {
        return Ok(None);
    };
    if row >= state.notification_history_len {
        return Ok(Some(focused_surface_id(state)));
    }
    state.overlay_selection = row;
    render_overlays_only(state)?;
    overlays::focus_notification_source(state).map(Some)
}

fn palette_click(state: &mut DesktopState, local_y: i32) -> rt::Result<Option<u32>> {
    let Some(row) = crate::chrome::overlay_row_at(local_y, crate::OVERLAY_RESULT_MAX) else {
        return Ok(None);
    };
    state.overlay_selection = row.min(crate::OVERLAY_RESULT_MAX - 1);
    overlays::handle_palette_key(state, KEY_ENTER).map(Some)
}
