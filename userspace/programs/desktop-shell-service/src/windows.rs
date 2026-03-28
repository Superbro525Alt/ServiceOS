use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;
use rt::{DesktopAppId, LogEvent, LogSeverity, StartupHandle};

use crate::{
    logging::{emit_log, emit_text_log},
    render::render_desktop,
    AppSlot, DesktopState, WindowState, APP_COUNT, PANEL_MARGIN, SESSION_ID, TOPBAR_HEIGHT,
    WINDOW_MIN_HEIGHT, WINDOW_MIN_WIDTH,
};

pub(crate) fn launch_or_focus_app(state: &mut DesktopState, app_id: DesktopAppId) -> rt::Result<u32> {
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Err(rt::Error::NotFound);
    };
    if state.apps[index].running {
        if state.apps[index].window.minimized {
            return restore_app(state, app_id);
        }
        return focus_app(state, app_id);
    }

    let (x, y, width, height, fill_rgb) = initial_window_layout(state.chrome.output_width, app_id);
    let z_order = allocate_z_order(state);
    let (surface_id, surface_handle) = rt::graphics_surface_create(
        state.graphics_handle,
        SESSION_ID,
        x,
        y,
        width,
        height,
        z_order,
        fill_rgb,
        false,
    )?;
    let surface_transfer = rt::handle_duplicate(
        surface_handle,
        rt::rights::SEND | rt::rights::RECEIVE | rt::rights::DUPLICATE | rt::rights::TRANSFER,
    )?;
    let control = rt::channel_create()?;

    let task_handle = rt::manager_launch_program_with_payload(
        state.bootstrap,
        state.apps[index].image_id,
        &[surface_id as u64, width as u64, height as u64, 1],
        &[
            StartupHandle {
                handle: surface_transfer,
                rights: rt::rights::SEND
                    | rt::rights::RECEIVE
                    | rt::rights::DUPLICATE
                    | rt::rights::TRANSFER,
            },
            StartupHandle {
                handle: control.second,
                rights: rt::rights::SEND
                    | rt::rights::RECEIVE
                    | rt::rights::DUPLICATE
                    | rt::rights::TRANSFER,
            },
        ],
    )?;
    let _ = rt::handle_close(surface_transfer);
    let _ = rt::handle_close(control.second);

    state.apps[index].task_handle = task_handle;
    state.apps[index].window = WindowState {
        surface_id,
        surface_handle,
        control_handle: control.first,
        x,
        y,
        width,
        height,
        z_order,
        minimized: false,
        maximized: false,
        restore_x: x,
        restore_y: y,
        restore_width: width,
        restore_height: height,
    };
    state.apps[index].running = true;
    sync_window_surface(&state.apps[index])?;
    let _ = rt::app_control_resize(control.first, width, height);
    let surface_id = focus_app(state, app_id)?;
    let _ = emit_log(
        state.log_handle,
        LogSeverity::Info,
        LogEvent::DesktopAppLaunched,
        app_id as u32 as u64,
        surface_id as u64,
    );
    Ok(surface_id)
}

pub(crate) fn focus_app(state: &mut DesktopState, app_id: DesktopAppId) -> rt::Result<u32> {
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Err(rt::Error::NotFound);
    };
    if !state.apps[index].running || state.apps[index].window.surface_id == 0 {
        return Err(rt::Error::NotFound);
    }
    if state.apps[index].window.minimized {
        state.apps[index].window.minimized = false;
        sync_window_surface(&state.apps[index])?;
    }

    if let Some(previous) = state.focused_app {
        if previous != app_id {
            if let Some(previous_index) = app_slot_index(&state.apps, previous) {
                let previous_control = state.apps[previous_index].window.control_handle;
                if previous_control != rt::INVALID_HANDLE {
                    let _ = rt::app_control_focus(previous_control, false);
                }
            }
        }
    }

    state.apps[index].window.z_order = allocate_z_order(state);
    apply_window_geometry(&state.apps[index])?;
    let control_handle = state.apps[index].window.control_handle;
    if control_handle != rt::INVALID_HANDLE {
        let _ = rt::app_control_focus(control_handle, true);
    }
    let surface_id = state.apps[index].window.surface_id;
    let _ = rt::session_focus(state.session_handle, SESSION_ID, surface_id)?;
    state.focused_app = Some(app_id);
    let _ = emit_log(
        state.log_handle,
        LogSeverity::Info,
        LogEvent::DesktopFocusChanged,
        app_id as u32 as u64,
        surface_id as u64,
    );
    render_desktop(state)?;
    Ok(surface_id)
}

pub(crate) fn minimize_app(state: &mut DesktopState, app_id: DesktopAppId) -> rt::Result<u32> {
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Err(rt::Error::NotFound);
    };
    if !state.apps[index].running {
        return Err(rt::Error::NotFound);
    }
    state.apps[index].window.minimized = true;
    sync_window_surface(&state.apps[index])?;
    if state.focused_app == Some(app_id) {
        state.focused_app = None;
        let _ = rt::session_focus(state.session_handle, SESSION_ID, 0);
        let _ = crate::input::focus_next_visible_without_cycle(state);
    }
    let _ = emit_text_log(
        "desktop",
        format_args!("window minimized app={}", app_title(app_id)),
    );
    render_desktop(state)?;
    Ok(state.apps[index].window.surface_id)
}

pub(crate) fn restore_app(state: &mut DesktopState, app_id: DesktopAppId) -> rt::Result<u32> {
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Err(rt::Error::NotFound);
    };
    if !state.apps[index].running {
        return Err(rt::Error::NotFound);
    }
    if state.apps[index].window.maximized {
        let restore_x = state.apps[index].window.restore_x;
        let restore_y = state.apps[index].window.restore_y;
        let restore_width = state.apps[index].window.restore_width.max(WINDOW_MIN_WIDTH);
        let restore_height = state.apps[index].window.restore_height.max(WINDOW_MIN_HEIGHT);
        state.apps[index].window.x =
            clamp_window_x(state.chrome.output_width, restore_width, restore_x);
        state.apps[index].window.y =
            clamp_window_y(state.chrome.output_height, restore_height, restore_y);
        state.apps[index].window.width = restore_width;
        state.apps[index].window.height = restore_height;
        state.apps[index].window.maximized = false;
        apply_window_geometry(&state.apps[index])?;
        if state.apps[index].window.control_handle != rt::INVALID_HANDLE {
            let _ = rt::app_control_resize(
                state.apps[index].window.control_handle,
                restore_width,
                restore_height,
            );
        }
    }
    state.apps[index].window.minimized = false;
    sync_window_surface(&state.apps[index])?;
    focus_app(state, app_id)
}

pub(crate) fn maximize_app(state: &mut DesktopState, app_id: DesktopAppId) -> rt::Result<u32> {
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Err(rt::Error::NotFound);
    };
    if !state.apps[index].running {
        return Err(rt::Error::NotFound);
    }
    if state.apps[index].window.maximized {
        return restore_app(state, app_id);
    }

    state.apps[index].window.restore_x = state.apps[index].window.x;
    state.apps[index].window.restore_y = state.apps[index].window.y;
    state.apps[index].window.restore_width = state.apps[index].window.width;
    state.apps[index].window.restore_height = state.apps[index].window.height;
    state.apps[index].window.maximized = true;
    state.apps[index].window.minimized = false;
    state.apps[index].window.x = PANEL_MARGIN as i32;
    state.apps[index].window.y = (TOPBAR_HEIGHT + PANEL_MARGIN) as i32;
    state.apps[index].window.width = state
        .chrome
        .output_width
        .saturating_sub(PANEL_MARGIN * 2);
    state.apps[index].window.height = state
        .chrome
        .output_height
        .saturating_sub(TOPBAR_HEIGHT + PANEL_MARGIN * 2);
    apply_window_geometry(&state.apps[index])?;
    if state.apps[index].window.control_handle != rt::INVALID_HANDLE {
        let _ = rt::app_control_resize(
            state.apps[index].window.control_handle,
            state.apps[index].window.width,
            state.apps[index].window.height,
        );
    }
    render_desktop(state)?;
    focus_app(state, app_id)
}

pub(crate) fn move_app(state: &mut DesktopState, app_id: DesktopAppId, x: i32, y: i32) -> rt::Result<u32> {
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Err(rt::Error::NotFound);
    };
    if !state.apps[index].running {
        return Err(rt::Error::NotFound);
    }
    state.apps[index].window.maximized = false;
    state.apps[index].window.x =
        clamp_window_x(state.chrome.output_width, state.apps[index].window.width, x);
    state.apps[index].window.y =
        clamp_window_y(state.chrome.output_height, state.apps[index].window.height, y);
    rt::surface_set_geometry_async(
        state.apps[index].window.surface_handle,
        state.apps[index].window.x,
        state.apps[index].window.y,
        state.apps[index].window.width,
        state.apps[index].window.height,
        state.apps[index].window.z_order,
    )?;
    Ok(state.apps[index].window.surface_id)
}

pub(crate) fn resize_app(
    state: &mut DesktopState,
    app_id: DesktopAppId,
    width: u32,
    height: u32,
) -> rt::Result<u32> {
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Err(rt::Error::NotFound);
    };
    if !state.apps[index].running {
        return Err(rt::Error::NotFound);
    }
    state.apps[index].window.maximized = false;
    let width = width.clamp(
        WINDOW_MIN_WIDTH,
        state.chrome.output_width.saturating_sub(PANEL_MARGIN),
    );
    let height = height.clamp(
        WINDOW_MIN_HEIGHT,
        state.chrome
            .output_height
            .saturating_sub(TOPBAR_HEIGHT + PANEL_MARGIN),
    );
    state.apps[index].window.width = width;
    state.apps[index].window.height = height;
    state.apps[index].window.x =
        clamp_window_x(state.chrome.output_width, width, state.apps[index].window.x);
    state.apps[index].window.y =
        clamp_window_y(state.chrome.output_height, height, state.apps[index].window.y);
    apply_window_geometry(&state.apps[index])?;
    let control_handle = state.apps[index].window.control_handle;
    if control_handle != rt::INVALID_HANDLE {
        let _ = rt::app_control_resize(control_handle, width, height);
    }
    render_desktop(state)?;
    let _ = emit_text_log(
        "desktop",
        format_args!("window resized app={} size={}x{}", app_title(app_id), width, height),
    );
    Ok(state.apps[index].window.surface_id)
}

pub(crate) fn close_app(state: &mut DesktopState, app_id: DesktopAppId) -> rt::Result<()> {
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Err(rt::Error::NotFound);
    };
    if !state.apps[index].running {
        return Err(rt::Error::NotFound);
    }
    let control_handle = state.apps[index].window.control_handle;
    if control_handle != rt::INVALID_HANDLE {
        let _ = rt::app_control_close(control_handle);
    }
    let _ = emit_text_log(
        "desktop",
        format_args!("window close requested app={}", app_title(app_id)),
    );
    Ok(())
}

pub(crate) fn refresh_apps(state: &mut DesktopState) -> rt::Result<()> {
    let mut changed = false;
    for slot in &mut state.apps {
        if !slot.running || slot.task_handle == rt::INVALID_HANDLE {
            continue;
        }
        let status = rt::task_status(slot.task_handle)?;
        if status.state != rt::TaskStateCode::Exited {
            continue;
        }
        let exited_app = slot.app_id;
        let exit_code = status.exit_code;
        if slot.window.surface_handle != rt::INVALID_HANDLE {
            let _ = rt::surface_close(slot.window.surface_handle);
            let _ = rt::handle_close(slot.window.surface_handle);
        }
        if slot.window.control_handle != rt::INVALID_HANDLE {
            let _ = rt::handle_close(slot.window.control_handle);
        }
        let _ = rt::handle_close(slot.task_handle);
        slot.task_handle = rt::INVALID_HANDLE;
        slot.window = WindowState::empty();
        slot.running = false;
        if state.focused_app == Some(exited_app) {
            state.focused_app = None;
            let _ = rt::session_focus(state.session_handle, SESSION_ID, 0);
        }
        let _ = emit_log(
            state.log_handle,
            LogSeverity::Warn,
            LogEvent::DesktopAppExited,
            exited_app as u32 as u64,
            exit_code,
        );
        changed = true;
    }
    if changed {
        let _ = crate::input::focus_next_visible_without_cycle(state);
        render_desktop(state)?;
    }
    Ok(())
}

pub(crate) fn encode_window_page(state: &DesktopState, start: usize, reply: &mut rt::RawMessage) {
    let mut windows = [WindowState::empty(); APP_COUNT];
    let mut app_ids = [DesktopAppId::Settings; APP_COUNT];
    let mut total = 0usize;
    for slot in state.apps.iter().copied() {
        if !slot.running || slot.window.surface_id == 0 {
            continue;
        }
        windows[total] = slot.window;
        app_ids[total] = slot.app_id;
        total += 1;
    }
    for index in 0..total {
        let mut best = index;
        for candidate in index + 1..total {
            if windows[candidate].z_order < windows[best].z_order {
                best = candidate;
            }
        }
        windows.swap(index, best);
        app_ids.swap(index, best);
    }

    let mut returned = 0usize;
    for index in start..total.min(start + crate::WINDOW_PAGE_SIZE) {
        let base = 3 + returned * 5;
        let app_id = app_ids[index];
        let window = windows[index];
        reply.words[base] = app_id as u32 as u64;
        reply.words[base + 1] = window.surface_id as u64;
        reply.words[base + 2] = pack_window_flags(
            window.z_order,
            state.focused_app == Some(app_id),
            window.minimized,
            window.visible(),
        );
        reply.words[base + 3] = pack_i32_pair(window.x, window.y);
        reply.words[base + 4] = pack_u32_pair(window.width, window.height);
        returned += 1;
    }
    reply.words[1] = returned as u64;
    reply.words[2] = if start + returned >= total {
        u32::MAX as u64
    } else {
        (start + returned) as u64
    };
    reply.word_count = (3 + returned * 5) as u32;
}

pub(crate) fn apply_window_geometry(slot: &AppSlot) -> rt::Result<()> {
    rt::surface_set_geometry(
        slot.window.surface_handle,
        slot.window.x,
        slot.window.y,
        slot.window.width,
        slot.window.height,
        slot.window.z_order,
    )
}

pub(crate) fn sync_window_surface(slot: &AppSlot) -> rt::Result<()> {
    apply_window_geometry(slot)?;
    rt::surface_set_visibility(slot.window.surface_handle, slot.window.visible())
}

pub(crate) fn allocate_z_order(state: &mut DesktopState) -> u32 {
    let z_order = state.next_z_order;
    state.next_z_order = state.next_z_order.saturating_add(1);
    z_order
}

pub(crate) fn focused_surface_id(state: &DesktopState) -> u32 {
    state
        .focused_app
        .and_then(|app_id| app_slot_index(&state.apps, app_id))
        .map(|index| state.apps[index].window.surface_id)
        .unwrap_or(0)
}

pub(crate) fn initial_window_layout(
    output_width: u32,
    app_id: DesktopAppId,
) -> (i32, i32, u32, u32, u32) {
    match app_id {
        DesktopAppId::Settings => (292, 92, 420, 240, ui::BG_WINDOW),
        DesktopAppId::Files => (336, 168, 560, 276, ui::BG_WINDOW_ALT),
        DesktopAppId::Monitor => (
            output_width.saturating_sub(500 + PANEL_MARGIN) as i32,
            108,
            480,
            240,
            ui::BG_WINDOW,
        ),
        DesktopAppId::Terminal => (220, 96, 720, 420, 0x11161f),
    }
}

pub(crate) fn clamp_window_x(output_width: u32, width: u32, requested: i32) -> i32 {
    let max_x = output_width.saturating_sub(width + PANEL_MARGIN) as i32;
    requested.clamp(PANEL_MARGIN as i32, max_x.max(PANEL_MARGIN as i32))
}

pub(crate) fn clamp_window_y(output_height: u32, height: u32, requested: i32) -> i32 {
    let min_y = (TOPBAR_HEIGHT + PANEL_MARGIN) as i32;
    let max_y = output_height
        .saturating_sub(height + PANEL_MARGIN)
        .max(TOPBAR_HEIGHT + PANEL_MARGIN) as i32;
    requested.clamp(min_y, max_y)
}

pub(crate) fn launcher_line(slot: AppSlot) -> &'static str {
    match (slot.app_id, slot.running, slot.window.minimized) {
        (DesktopAppId::Settings, true, false) => "SETTINGS RUN",
        (DesktopAppId::Settings, true, true) => "SETTINGS MIN",
        (DesktopAppId::Settings, false, _) => "SETTINGS OFF",
        (DesktopAppId::Files, true, false) => "FILES RUN",
        (DesktopAppId::Files, true, true) => "FILES MIN",
        (DesktopAppId::Files, false, _) => "FILES OFF",
        (DesktopAppId::Monitor, true, false) => "MONITOR RUN",
        (DesktopAppId::Monitor, true, true) => "MONITOR MIN",
        (DesktopAppId::Monitor, false, _) => "MONITOR OFF",
        (DesktopAppId::Terminal, true, false) => "TERMINAL RUN",
        (DesktopAppId::Terminal, true, true) => "TERMINAL MIN",
        (DesktopAppId::Terminal, false, _) => "TERMINAL OFF",
    }
}

pub(crate) fn running_app_count(apps: &[AppSlot; APP_COUNT]) -> usize {
    apps.iter().filter(|slot| slot.running).count()
}

pub(crate) fn app_slot_index(apps: &[AppSlot; APP_COUNT], app_id: DesktopAppId) -> Option<usize> {
    apps.iter().position(|slot| slot.app_id == app_id)
}

pub(crate) fn app_title(app_id: DesktopAppId) -> &'static str {
    match app_id {
        DesktopAppId::Settings => "SETTINGS",
        DesktopAppId::Files => "FILES",
        DesktopAppId::Monitor => "MONITOR",
        DesktopAppId::Terminal => "TERMINAL",
    }
}

pub(crate) fn pack_window_flags(z_order: u32, focused: bool, minimized: bool, visible: bool) -> u64 {
    let mut flags = (z_order as u64) << 32;
    if focused {
        flags |= 0x1;
    }
    if minimized {
        flags |= 0x2;
    }
    if visible {
        flags |= 0x4;
    }
    flags
}

pub(crate) fn pack_i32_pair(first: i32, second: i32) -> u64 {
    (first as u32 as u64) | ((second as u32 as u64) << 32)
}

pub(crate) fn pack_u32_pair(first: u32, second: u32) -> u64 {
    first as u64 | ((second as u64) << 32)
}
