use super::*;

pub(super) fn handle_key_input(
    state: &mut DesktopState,
    action: AppKeyAction,
    key_code: u32,
    modifiers: u32,
) -> rt::Result<u32> {
    if action == AppKeyAction::Up
        && state.overlay_mode == OverlayMode::Switcher
        && (key_code == KEY_LEFT_ALT || key_code == KEY_RIGHT_ALT)
    {
        state.overlay_mode = OverlayMode::None;
        state.overlay_selection = 0;
        return Ok(focused_surface_id(state));
    }

    if action == AppKeyAction::Down {
        if key_code == KEY_ESC && state.overlay_mode != OverlayMode::None {
            state.overlay_mode = OverlayMode::None;
            state.overlay_selection = 0;
            state.palette_query_len = 0;
            return Ok(focused_surface_id(state));
        }

        if modifiers & MOD_CTRL != 0 && modifiers & MOD_ALT != 0 {
            let workspace = match key_code {
                KEY_1 => Some(1),
                KEY_2 => Some(2),
                KEY_3 => Some(3),
                KEY_4 => Some(4),
                _ => None,
            };
            if let Some(workspace_id) = workspace {
                if modifiers & MOD_SHIFT != 0 {
                    return move_focused_to_workspace(state, workspace_id);
                }
                return switch_workspace(state, workspace_id);
            }
            if key_code == KEY_V {
                state.overlay_mode = OverlayMode::ClipboardHistory;
                state.overlay_selection = 0;
                return Ok(focused_surface_id(state));
            }
        }

        if modifiers & MOD_CTRL != 0 && key_code == KEY_SPACE {
            state.overlay_mode = OverlayMode::CommandPalette;
            state.overlay_selection = 0;
            state.palette_query_len = 0;
            return Ok(focused_surface_id(state));
        }

        if modifiers & MOD_ALT != 0 && key_code == KEY_N {
            state.overlay_mode = OverlayMode::Notifications;
            state.overlay_selection = 0;
            return Ok(focused_surface_id(state));
        }

        if state.overlay_mode == OverlayMode::CommandPalette {
            return overlays::handle_palette_key(state, key_code);
        }
        if state.overlay_mode == OverlayMode::Notifications {
            return overlays::handle_notification_overlay_key(state, key_code);
        }
        if state.overlay_mode == OverlayMode::ClipboardHistory {
            return overlays::handle_clipboard_overlay_key(state, key_code);
        }
    }

    if action == AppKeyAction::Down && modifiers & MOD_ALT != 0 {
        if key_code == KEY_TAB {
            state.overlay_mode = OverlayMode::Switcher;
            if modifiers & MOD_SHIFT != 0 {
                return focus_previous_app(state);
            }
            return focus_next_app(state);
        }
        if key_code == KEY_F4 {
            if let Some(app_id) = state.focused_app {
                close_app(state, app_id)?;
                return Ok(focused_surface_id(state));
            }
        }
    }
    if action == AppKeyAction::Down && modifiers & MOD_ALT != 0 {
        let direct = match key_code {
            KEY_1 => Some(DesktopAppId::Settings),
            KEY_2 => Some(DesktopAppId::Files),
            KEY_3 => Some(DesktopAppId::Monitor),
            KEY_4 => Some(DesktopAppId::Terminal),
            KEY_5 => Some(DesktopAppId::SoftwareCenter),
            _ => None,
        };
        if let Some(app_id) = direct {
            return crate::windows::launch_or_focus_app(state, app_id);
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

pub(super) fn handle_text_input(state: &mut DesktopState, scalar: u32) -> rt::Result<u32> {
    let Some(ch) = core::char::from_u32(scalar) else {
        return Ok(focused_surface_id(state));
    };
    if state.overlay_mode == OverlayMode::CommandPalette {
        if !ch.is_control() && state.palette_query_len < state.palette_query.len() {
            state.palette_query[state.palette_query_len] = ch as u8;
            state.palette_query_len += 1;
            state.overlay_selection = 0;
        }
        return Ok(focused_surface_id(state));
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
    rt::app_control_text(control, ch)?;
    Ok(state.apps[index].window.surface_id)
}
