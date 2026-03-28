use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;
use rt::{AppKeyAction, AppPointerAction, DesktopAppId, DesktopInputAction, LogEvent, LogSeverity};

use crate::{
    logging::emit_log,
    render::{render_desktop, sync_cursor},
    windows::{
        app_slot_index, clamp_window_x, clamp_window_y, close_app, focus_app,
        focused_surface_id, maximize_app, minimize_app, move_app,
    },
    ContentCapture, DesktopState, DragState, HitTarget, ResizeEdges, WindowState, APP_COUNT,
    KEY_F4, KEY_TAB, MOD_ALT, PANEL_MARGIN, RESIZE_GRIP_SIZE, TOPBAR_HEIGHT, WINDOW_MIN_HEIGHT,
    WINDOW_MIN_WIDTH,
};

pub(crate) fn handle_input(
    state: &mut DesktopState,
    action: DesktopInputAction,
    x: i32,
    y: i32,
) -> rt::Result<u32> {
    let result = match action {
        DesktopInputAction::PointerDown => {
            state.pointer_x = x;
            state.pointer_y = y;
            handle_pointer_down(state, x, y)
        }
        DesktopInputAction::PointerMove => {
            state.pointer_x = x;
            state.pointer_y = y;
            handle_pointer_move(state, x, y)
        }
        DesktopInputAction::PointerUp => {
            state.pointer_x = x;
            state.pointer_y = y;
            let surface_id = handle_pointer_up(state, x, y)?;
            state.drag_state = None;
            state.content_capture = None;
            Ok(surface_id)
        }
        DesktopInputAction::Click => {
            state.pointer_x = x;
            state.pointer_y = y;
            let _ = handle_pointer_down(state, x, y)?;
            state.drag_state = None;
            state.content_capture = None;
            Ok(focused_surface_id(state))
        }
        DesktopInputAction::KeyDown => handle_key_input(state, AppKeyAction::Down, x as u32, y as u32),
        DesktopInputAction::KeyUp => handle_key_input(state, AppKeyAction::Up, x as u32, y as u32),
        DesktopInputAction::TextInput => handle_text_input(state, x as u32),
    }?;
    sync_cursor(state)?;
    match action {
        DesktopInputAction::PointerMove => {}
        _ => render_desktop(state)?,
    }
    Ok(result)
}

fn handle_pointer_down(state: &mut DesktopState, x: i32, y: i32) -> rt::Result<u32> {
    match hit_test(state, x, y) {
        HitTarget::Background => {
            state.drag_state = None;
            state.content_capture = None;
            Ok(focused_surface_id(state))
        }
        HitTarget::Launcher(app_id) => crate::windows::launch_or_focus_app(state, app_id),
        HitTarget::WindowContent(app_id) => {
            state.drag_state = None;
            state.content_capture = Some(ContentCapture { app_id, button: 1 });
            let surface_id = focus_app(state, app_id)?;
            let (local_x, local_y) = app_local_coords(state, app_id, x, y)?;
            dispatch_pointer_to_app(state, app_id, AppPointerAction::Down, local_x, local_y, 1)?;
            Ok(surface_id)
        }
        HitTarget::WindowMove {
            app_id,
            grab_offset_x,
            grab_offset_y,
        } => {
            state.content_capture = None;
            let surface_id = focus_app(state, app_id)?;
            state.drag_state = Some(DragState::Move {
                app_id,
                grab_offset_x,
                grab_offset_y,
            });
            Ok(surface_id)
        }
        HitTarget::WindowResize { app_id, edges } => {
            state.content_capture = None;
            let surface_id = focus_app(state, app_id)?;
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

fn handle_pointer_move(state: &mut DesktopState, x: i32, y: i32) -> rt::Result<u32> {
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
                )?;
            }
            Ok(focused_surface_id(state))
        }
    }
}

fn handle_pointer_up(state: &mut DesktopState, x: i32, y: i32) -> rt::Result<u32> {
    if let Some(capture) = state.content_capture {
        let (local_x, local_y) = app_local_coords(state, capture.app_id, x, y)?;
        dispatch_pointer_to_app(
            state,
            capture.app_id,
            AppPointerAction::Up,
            local_x,
            local_y,
            capture.button,
        )?;
    }
    Ok(focused_surface_id(state))
}

fn handle_key_input(
    state: &mut DesktopState,
    action: AppKeyAction,
    key_code: u32,
    modifiers: u32,
) -> rt::Result<u32> {
    if action == AppKeyAction::Down && modifiers & MOD_ALT != 0 {
        if key_code == KEY_TAB {
            return focus_next_app(state);
        }
        if key_code == KEY_F4 {
            if let Some(app_id) = state.focused_app {
                close_app(state, app_id)?;
                return Ok(focused_surface_id(state));
            }
        }
    }

    let Some(app_id) = state.focused_app else {
        return Ok(0);
    };
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Ok(0);
    };
    let control = state.apps[index].window.control_handle;
    if control == rt::INVALID_HANDLE {
        return Ok(0);
    }
    rt::app_control_key(control, action, key_code, modifiers)?;
    let _ = emit_log(
        state.log_handle,
        LogSeverity::Debug,
        LogEvent::InputKeyDelivered,
        app_id as u32 as u64,
        key_code as u64,
    );
    Ok(state.apps[index].window.surface_id)
}

fn handle_text_input(state: &mut DesktopState, scalar: u32) -> rt::Result<u32> {
    let Some(ch) = core::char::from_u32(scalar) else {
        return Ok(focused_surface_id(state));
    };
    let Some(app_id) = state.focused_app else {
        return Ok(0);
    };
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Ok(0);
    };
    let control = state.apps[index].window.control_handle;
    if control == rt::INVALID_HANDLE {
        return Ok(0);
    }
    rt::app_control_text(control, ch)?;
    Ok(state.apps[index].window.surface_id)
}

fn sort_app_ids_by_z(state: &DesktopState, values: &mut [DesktopAppId]) {
    let mut index = 1usize;
    while index < values.len() {
        let current = values[index];
        let current_z = app_slot_index(&state.apps, current)
            .map(|slot_index| state.apps[slot_index].window.z_order)
            .unwrap_or(0);
        let mut scan = index;
        while scan > 0 {
            let prev = values[scan - 1];
            let prev_z = app_slot_index(&state.apps, prev)
                .map(|slot_index| state.apps[slot_index].window.z_order)
                .unwrap_or(0);
            if prev_z <= current_z {
                break;
            }
            values[scan] = prev;
            scan -= 1;
        }
        values[scan] = current;
        index += 1;
    }
}

fn hit_test(state: &DesktopState, x: i32, y: i32) -> HitTarget {
    if let Some(app_id) = launcher_hit_app(state, x, y) {
        return HitTarget::Launcher(app_id);
    }

    let mut order = [DesktopAppId::Settings; APP_COUNT];
    let mut count = 0usize;
    for slot in state.apps.iter().copied() {
        if slot.running && slot.window.visible() {
            order[count] = slot.app_id;
            count += 1;
        }
    }
    sort_app_ids_by_z(state, &mut order[..count]);

    for app_id in order[..count].iter().copied().rev() {
        let index = app_slot_index(&state.apps, app_id).unwrap();
        let window = state.apps[index].window;
        if x < window.x
            || y < window.y
            || x >= window.x + window.width as i32
            || y >= window.y + window.height as i32
        {
            continue;
        }

        let local_x = x - window.x;
        let local_y = y - window.y;
        if local_y < ui::TITLEBAR_HEIGHT as i32 {
            let close_left =
                window.width as i32 - ui::WINDOW_BUTTON_RIGHT_MARGIN - ui::WINDOW_BUTTON_SIZE as i32;
            let minimize_left = close_left - ui::WINDOW_BUTTON_GAP - ui::WINDOW_BUTTON_SIZE as i32;
            let maximize_left = minimize_left - ui::WINDOW_BUTTON_GAP - ui::WINDOW_BUTTON_SIZE as i32;
            if local_x >= close_left && local_x < close_left + ui::WINDOW_BUTTON_SIZE as i32 {
                return HitTarget::WindowClose(app_id);
            }
            if local_x >= minimize_left
                && local_x < minimize_left + ui::WINDOW_BUTTON_SIZE as i32
            {
                return HitTarget::WindowMinimize(app_id);
            }
            if local_x >= maximize_left
                && local_x < maximize_left + ui::WINDOW_BUTTON_SIZE as i32
            {
                return HitTarget::WindowMaximize(app_id);
            }
        }

        let resize_edges = resize_hit_edges(&window, local_x, local_y);
        if !resize_edges.is_empty() && !window.maximized {
            return HitTarget::WindowResize { app_id, edges: resize_edges };
        }

        if local_y < ui::TITLEBAR_HEIGHT as i32 {
            return HitTarget::WindowMove {
                app_id,
                grab_offset_x: local_x,
                grab_offset_y: local_y,
            };
        }

        return HitTarget::WindowContent(app_id);
    }

    HitTarget::Background
}

fn launcher_hit_app(state: &DesktopState, x: i32, y: i32) -> Option<DesktopAppId> {
    let launcher_x = PANEL_MARGIN as i32;
    let launcher_y = (TOPBAR_HEIGHT + PANEL_MARGIN) as i32;
    if x < launcher_x
        || y < launcher_y
        || x >= launcher_x + crate::LAUNCHER_WIDTH as i32
        || y >= launcher_y + 278
    {
        return None;
    }

    let local_y = y - launcher_y;
    for index in 0..APP_COUNT {
        let line_y = ui::PANEL_LINE_START_Y + ((index as i32 + 1) * ui::PANEL_LINE_STEP);
        let line_top = line_y - 2;
        let line_bottom = line_top + ui::PANEL_LINE_STEP;
        if local_y >= line_top && local_y < line_bottom {
            return Some(state.apps[index].app_id);
        }
    }
    None
}

pub(crate) fn focus_next_app(state: &mut DesktopState) -> rt::Result<u32> {
    let mut candidates = [DesktopAppId::Settings; APP_COUNT];
    let mut count = 0usize;
    for slot in state.apps.iter().copied() {
        if slot.running && slot.window.visible() {
            candidates[count] = slot.app_id;
            count += 1;
        }
    }
    if count == 0 {
        return Err(rt::Error::NotFound);
    }
    sort_app_ids_by_z(state, &mut candidates[..count]);

    let next = if let Some(current) = state.focused_app {
        let current_index = candidates[..count]
            .iter()
            .position(|candidate| *candidate == current)
            .unwrap_or(usize::MAX);
        if current_index == usize::MAX {
            candidates[count - 1]
        } else {
            candidates[(current_index + 1) % count]
        }
    } else {
        candidates[count - 1]
    };
    focus_app(state, next)
}

pub(crate) fn focus_next_visible_without_cycle(state: &mut DesktopState) -> rt::Result<u32> {
    let mut best: Option<(u32, DesktopAppId)> = None;
    for slot in state.apps.iter().copied() {
        if !slot.running || !slot.window.visible() {
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

pub(crate) fn dispatch_pointer_to_app(
    state: &DesktopState,
    app_id: DesktopAppId,
    action: AppPointerAction,
    local_x: i32,
    local_y: i32,
    button: u32,
) -> rt::Result<()> {
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Err(rt::Error::NotFound);
    };
    let control = state.apps[index].window.control_handle;
    if control == rt::INVALID_HANDLE {
        return Err(rt::Error::NotFound);
    }
    rt::app_control_pointer(control, action, local_x, local_y, button)
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

fn resize_hit_edges(window: &WindowState, local_x: i32, local_y: i32) -> ResizeEdges {
    let mut edges = ResizeEdges::NONE;
    if local_x <= ui::WINDOW_BORDER_THICKNESS {
        edges |= ResizeEdges::LEFT;
    }
    if local_x >= window.width as i32 - RESIZE_GRIP_SIZE {
        edges |= ResizeEdges::RIGHT;
    }
    if local_y <= ui::WINDOW_BORDER_THICKNESS {
        edges |= ResizeEdges::TOP;
    }
    if local_y >= window.height as i32 - RESIZE_GRIP_SIZE {
        edges |= ResizeEdges::BOTTOM;
    }
    edges
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
        let _ = rt::app_control_resize(
            state.apps[index].window.control_handle,
            state.apps[index].window.width,
            state.apps[index].window.height,
        );
    }
    Ok(state.apps[index].window.surface_id)
}
