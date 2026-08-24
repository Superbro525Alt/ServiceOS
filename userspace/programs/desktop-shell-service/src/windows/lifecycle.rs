use super::*;
use crate::render::render_desktop;

fn clear_pending_resize(state: &mut DesktopState, app_id: DesktopAppId) {
    if state
        .pending_resize
        .is_some_and(|pending| pending.app_id == app_id)
    {
        state.pending_resize = None;
    }
}

pub(crate) fn launch_or_focus_app(
    state: &mut DesktopState,
    app_id: DesktopAppId,
) -> rt::Result<u32> {
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
    state.apps[index].workspace_id = state.active_workspace;
    state.apps[index].launch_count = state.apps[index].launch_count.saturating_add(1);
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
    let surface_id = focus_app_internal(state, app_id, false, false)?;
    begin_open_animation(state, app_id)?;
    state.pending_shell_refresh.set();
    let _ = emit_log(
        state.log_handle,
        LogSeverity::Info,
        LogEvent::DesktopAppLaunched,
        app_id as u32 as u64,
        surface_id as u64,
    );
    Ok(surface_id)
}

pub(crate) fn schedule_launch_or_focus_app(
    state: &mut DesktopState,
    app_id: DesktopAppId,
) -> rt::Result<u32> {
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Err(rt::Error::NotFound);
    };
    if state.apps[index].running {
        if state.apps[index].window.minimized {
            return restore_app(state, app_id);
        }
        return focus_app(state, app_id);
    }
    state.pending_app_launch.replace(app_id);
    Ok(focused_surface_id(state))
}

pub(crate) fn focus_app(state: &mut DesktopState, app_id: DesktopAppId) -> rt::Result<u32> {
    focus_app_internal(state, app_id, true, true)
}

fn focus_app_internal(
    state: &mut DesktopState,
    app_id: DesktopAppId,
    rerender: bool,
    update_z_order: bool,
) -> rt::Result<u32> {
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Err(rt::Error::NotFound);
    };
    if !state.apps[index].running || state.apps[index].window.surface_id == 0 {
        return Err(rt::Error::NotFound);
    }
    if state.apps[index].window.minimized {
        state.apps[index].window.minimized = false;
        let _ = set_window_visibility(&state.apps[index], true);
    }
    if state.apps[index].workspace_id != state.active_workspace {
        state.active_workspace = state.apps[index].workspace_id.clamp(1, WORKSPACE_COUNT);
        sync_workspace_visibility(state)?;
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

    if update_z_order {
        state.apps[index].window.z_order = allocate_z_order(state);
        apply_window_geometry_async(&state.apps[index])?;
    }
    let control_handle = state.apps[index].window.control_handle;
    if control_handle != rt::INVALID_HANDLE {
        let _ = rt::app_control_focus(control_handle, true);
    }
    let surface_id = state.apps[index].window.surface_id;
    let _ = rt::session_focus(state.session_handle, SESSION_ID, surface_id)?;
    state.focused_app = Some(app_id);
    push_recent_focus(state, app_id);
    let _ = emit_log(
        state.log_handle,
        LogSeverity::Info,
        LogEvent::DesktopFocusChanged,
        app_id as u32 as u64,
        surface_id as u64,
    );
    if rerender {
        state.pending_focus_refresh.set();
    }
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
    begin_minimize_animation(state, app_id)?;
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
    let was_minimized = state.apps[index].window.minimized;
    if state.apps[index].window.maximized {
        let restore_x = state.apps[index].window.restore_x;
        let restore_y = state.apps[index].window.restore_y;
        let restore_width = state.apps[index].window.restore_width.max(WINDOW_MIN_WIDTH);
        let restore_height = state.apps[index]
            .window
            .restore_height
            .max(WINDOW_MIN_HEIGHT);
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
    let surface_id = focus_app_internal(state, app_id, true, true)?;
    if was_minimized {
        begin_restore_animation(state, app_id)?;
    }
    Ok(surface_id)
}

pub(crate) fn maximize_app(state: &mut DesktopState, app_id: DesktopAppId) -> rt::Result<u32> {
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Err(rt::Error::NotFound);
    };
    if !state.apps[index].running {
        return Err(rt::Error::NotFound);
    }
    cancel_animations(&mut state.animations, app_id);
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
    state.apps[index].window.width = state.chrome.output_width.saturating_sub(PANEL_MARGIN * 2);
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
    focus_app_internal(state, app_id, true, true)
}

pub(crate) fn move_app(
    state: &mut DesktopState,
    app_id: DesktopAppId,
    x: i32,
    y: i32,
) -> rt::Result<u32> {
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Err(rt::Error::NotFound);
    };
    if !state.apps[index].running {
        return Err(rt::Error::NotFound);
    }
    cancel_animations(&mut state.animations, app_id);
    state.apps[index].window.maximized = false;
    state.apps[index].window.x =
        clamp_window_x(state.chrome.output_width, state.apps[index].window.width, x);
    state.apps[index].window.y = clamp_window_y(
        state.chrome.output_height,
        state.apps[index].window.height,
        y,
    );
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
    cancel_animations(&mut state.animations, app_id);
    state.apps[index].window.maximized = false;
    let width = width.clamp(
        WINDOW_MIN_WIDTH,
        state.chrome.output_width.saturating_sub(PANEL_MARGIN),
    );
    let height = height.clamp(
        WINDOW_MIN_HEIGHT,
        state
            .chrome
            .output_height
            .saturating_sub(TOPBAR_HEIGHT + PANEL_MARGIN),
    );
    state.apps[index].window.width = width;
    state.apps[index].window.height = height;
    state.apps[index].window.x =
        clamp_window_x(state.chrome.output_width, width, state.apps[index].window.x);
    state.apps[index].window.y = clamp_window_y(
        state.chrome.output_height,
        height,
        state.apps[index].window.y,
    );
    apply_window_geometry(&state.apps[index])?;
    let control_handle = state.apps[index].window.control_handle;
    if control_handle != rt::INVALID_HANDLE {
        let _ = rt::app_control_resize(control_handle, width, height);
    }
    render_desktop(state)?;
    let _ = emit_text_log(
        "desktop",
        format_args!(
            "window resized app={} size={}x{}",
            app_title(app_id),
            width,
            height
        ),
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
    clear_pending_resize(state, app_id);
    cancel_animations(&mut state.animations, app_id);
    begin_close_animation(state, app_id)?;
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
    let mut pending_fault_notice: Option<(DesktopAppId, FixedLogBuffer<MAX_NOTIFICATION_BYTES>)> =
        None;
    let mut exited_app_to_clear: Option<DesktopAppId> = None;
    for index in 0..state.apps.len() {
        if !state.apps[index].running || state.apps[index].task_handle == rt::INVALID_HANDLE {
            continue;
        }
        let status = rt::task_status(state.apps[index].task_handle)?;
        if !matches!(
            status.state,
            rt::TaskStateCode::Exited | rt::TaskStateCode::Faulted
        ) {
            continue;
        }
        let exited_app = state.apps[index].app_id;
        let exit_code = status.exit_code;
        let faulted = status.state == rt::TaskStateCode::Faulted;
        cancel_animations(&mut state.animations, exited_app);
        if state.apps[index].window.surface_handle != rt::INVALID_HANDLE {
            let _ = rt::surface_close(state.apps[index].window.surface_handle);
            let _ = rt::handle_close(state.apps[index].window.surface_handle);
        }
        if state.apps[index].window.control_handle != rt::INVALID_HANDLE {
            let _ = rt::handle_close(state.apps[index].window.control_handle);
        }
        let _ = rt::handle_close(state.apps[index].task_handle);
        state.apps[index].task_handle = rt::INVALID_HANDLE;
        state.apps[index].window = WindowState::empty();
        state.apps[index].running = false;
        exited_app_to_clear = Some(exited_app);
        if state.focused_app == Some(exited_app) {
            state.focused_app = None;
            let _ = rt::session_focus(state.session_handle, SESSION_ID, 0);
        }
        let _ = emit_log(
            state.log_handle,
            if faulted {
                LogSeverity::Error
            } else {
                LogSeverity::Warn
            },
            LogEvent::DesktopAppExited,
            exited_app as u32 as u64,
            exit_code,
        );
        if faulted {
            let mut message = FixedLogBuffer::<MAX_NOTIFICATION_BYTES>::new();
            let _ = write!(
                &mut message,
                "{} faulted ({:#x})",
                app_title(exited_app),
                exit_code
            );
            pending_fault_notice = Some((exited_app, message));
        }
        changed = true;
    }
    if let Some(exited_app) = exited_app_to_clear {
        clear_pending_resize(state, exited_app);
    }
    if let Some((app_id, message)) = pending_fault_notice {
        post_notification(state, Some(app_id), true, message.as_bytes())?;
    }
    if changed {
        let _ = crate::input::focus_next_visible_without_cycle(state);
        render_desktop(state)?;
    }
    Ok(())
}

pub(crate) fn flush_pending_resize(state: &mut DesktopState) -> rt::Result<()> {
    let Some(pending) = state.pending_resize else {
        return Ok(());
    };
    let Some(index) = app_slot_index(&state.apps, pending.app_id) else {
        state.pending_resize = None;
        return Ok(());
    };
    let control_handle = state.apps[index].window.control_handle;
    if control_handle == rt::INVALID_HANDLE || !state.apps[index].running {
        state.pending_resize = None;
        return Ok(());
    }
    match rt::app_control_resize(control_handle, pending.width, pending.height) {
        Ok(()) => {
            state.pending_resize = None;
            Ok(())
        }
        Err(rt::Error::CapacityExceeded | rt::Error::Busy) => Ok(()),
        Err(error) => Err(error),
    }
}

pub(crate) fn switch_workspace(state: &mut DesktopState, workspace_id: u32) -> rt::Result<u32> {
    let workspace_id = workspace_id.clamp(1, WORKSPACE_COUNT);
    if state.active_workspace == workspace_id {
        return Ok(focused_surface_id(state));
    }
    if let Some(current) = state.focused_app {
        if let Some(index) = app_slot_index(&state.apps, current) {
            let control = state.apps[index].window.control_handle;
            if control != rt::INVALID_HANDLE {
                let _ = rt::app_control_focus(control, false);
            }
        }
    }
    state.focused_app = None;
    state.active_workspace = workspace_id;
    sync_workspace_visibility(state)?;
    let surface_id = crate::input::focus_next_visible_without_cycle(state)?;
    render_desktop(state)?;
    Ok(surface_id)
}

pub(crate) fn move_focused_to_workspace(
    state: &mut DesktopState,
    workspace_id: u32,
) -> rt::Result<u32> {
    let Some(app_id) = state.focused_app else {
        return Err(rt::Error::NotFound);
    };
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Err(rt::Error::NotFound);
    };
    state.apps[index].workspace_id = workspace_id.clamp(1, WORKSPACE_COUNT);
    sync_workspace_visibility(state)?;
    if state.apps[index].workspace_id != state.active_workspace {
        state.focused_app = None;
        let _ = rt::session_focus(state.session_handle, SESSION_ID, 0);
        let _ = crate::input::focus_next_visible_without_cycle(state)?;
    }
    render_desktop(state)?;
    Ok(focused_surface_id(state))
}

pub(crate) fn open_path_in_files(state: &mut DesktopState, path: &str) -> rt::Result<u32> {
    let surface_id = launch_or_focus_app(state, DesktopAppId::Files)?;
    let Some(index) = app_slot_index(&state.apps, DesktopAppId::Files) else {
        return Err(rt::Error::NotFound);
    };
    let control = state.apps[index].window.control_handle;
    if control == rt::INVALID_HANDLE {
        return Err(rt::Error::NotFound);
    }
    rt::app_control_open_path(control, path)?;
    Ok(surface_id)
}
