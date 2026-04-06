use super::*;

pub(super) fn handle_pointer_down(state: &mut DesktopState, x: i32, y: i32) -> rt::Result<u32> {
    if state.overlay_mode != OverlayMode::None {
        state.overlay_mode = OverlayMode::None;
        state.overlay_selection = 0;
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
        HitTarget::WindowContent(app_id) => {
            state.drag_state = None;
            state.content_capture = Some(ContentCapture { app_id, button: 1 });
            let surface_id = if state.focused_app == Some(app_id) {
                focused_surface_id(state)
            } else {
                focus_app(state, app_id)?
            };
            let (local_x, local_y) = app_local_coords(state, app_id, x, y)?;
            dispatch_pointer_to_app(state, app_id, AppPointerAction::Down, local_x, local_y, 1, 0)?;
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
        }) => move_app(state, app_id, x - grab_offset_x, y - grab_offset_y),
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
    if let Some(DragState::Resize { .. }) = state.drag_state {
        crate::windows::flush_pending_resize(state)?;
    }
    Ok(focused_surface_id(state))
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
    Ok(((x - window.x).clamp(0, max_x), (y - window.y).clamp(0, max_y)))
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
    state.apps[index].window.x =
        clamp_window_x(state.chrome.output_width, new_width as u32, new_x);
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
