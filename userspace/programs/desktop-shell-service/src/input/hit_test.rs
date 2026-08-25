use super::*;

pub(super) fn hit_test(state: &DesktopState, x: i32, y: i32) -> HitTarget {
    if let Some(app_id) = launcher_hit_app(state, x, y) {
        return HitTarget::Launcher(app_id);
    }

    let mut order = [DesktopAppId::Settings; APP_COUNT];
    let mut count = 0usize;
    for slot in state.apps.iter().copied() {
        if slot.running && slot.window.visible() && slot.workspace_id == state.active_workspace {
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
            let close_left = window.width as i32
                - ui::WINDOW_BUTTON_RIGHT_MARGIN
                - ui::WINDOW_BUTTON_SIZE as i32;
            let minimize_left = close_left - ui::WINDOW_BUTTON_GAP - ui::WINDOW_BUTTON_SIZE as i32;
            let maximize_left =
                minimize_left - ui::WINDOW_BUTTON_GAP - ui::WINDOW_BUTTON_SIZE as i32;
            if local_x >= close_left && local_x < close_left + ui::WINDOW_BUTTON_SIZE as i32 {
                return HitTarget::WindowClose(app_id);
            }
            if local_x >= minimize_left && local_x < minimize_left + ui::WINDOW_BUTTON_SIZE as i32 {
                return HitTarget::WindowMinimize(app_id);
            }
            if local_x >= maximize_left && local_x < maximize_left + ui::WINDOW_BUTTON_SIZE as i32 {
                return HitTarget::WindowMaximize(app_id);
            }
        }

        let resize_edges = resize_hit_edges(&window, local_x, local_y);
        if !resize_edges.is_empty() && !window.maximized {
            return HitTarget::WindowResize {
                app_id,
                edges: resize_edges,
            };
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

fn launcher_hit_app(state: &DesktopState, x: i32, y: i32) -> Option<DesktopAppId> {
    let (launcher_x, launcher_y, launcher_w, launcher_h) = crate::access::launcher_base_rect(state);
    if x < launcher_x
        || y < launcher_y
        || x >= launcher_x + launcher_w as i32
        || y >= launcher_y + launcher_h as i32
    {
        return None;
    }

    let local_y = y - launcher_y;
    for index in 0..APP_COUNT {
        let line_y = ui::PANEL_LINE_START_Y + (index as i32 * ui::PANEL_LINE_STEP);
        let line_top = line_y - 2;
        let line_bottom = line_top + ui::PANEL_LINE_STEP;
        if local_y >= line_top && local_y < line_bottom {
            return Some(state.apps[index].app_id);
        }
    }
    None
}

/// Launcher icon currently under the pointer, used for drop-target
/// highlighting while a content drag is armed.
pub(crate) fn launcher_hover_app(state: &DesktopState) -> Option<DesktopAppId> {
    launcher_hit_app(state, state.pointer_x, state.pointer_y)
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
