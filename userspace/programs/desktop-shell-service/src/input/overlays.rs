use super::*;

pub(super) fn focus_recent_app(state: &mut DesktopState, offset: usize) -> rt::Result<u32> {
    let mut candidates = [DesktopAppId::Settings; APP_COUNT];
    let mut count = 0usize;
    for app_id in state.recent_focus[..state.recent_focus_len].iter().copied() {
        if visible_on_workspace(state, app_id) {
            candidates[count] = app_id;
            count += 1;
        }
    }
    if count == 0 {
        return Err(rt::Error::NotFound);
    }
    let current_index = state
        .focused_app
        .and_then(|app_id| candidates[..count].iter().position(|candidate| *candidate == app_id))
        .unwrap_or(0);
    let next = candidates[(current_index + offset) % count];
    focus_app(state, next)
}

pub(super) fn handle_palette_key(state: &mut DesktopState, key_code: u32) -> rt::Result<u32> {
    let mut results = [PaletteAction::ShowNotifications; OVERLAY_RESULT_MAX];
    let count = palette_matches(state, &mut results);
    match key_code {
        KEY_BACKSPACE => {
            if state.palette_query_len > 0 {
                state.palette_query_len -= 1;
                state.overlay_selection = 0;
            }
            Ok(focused_surface_id(state))
        }
        KEY_UP => {
            if count != 0 {
                state.overlay_selection = state.overlay_selection.saturating_sub(1);
            }
            Ok(focused_surface_id(state))
        }
        KEY_DOWN => {
            if count != 0 {
                state.overlay_selection = (state.overlay_selection + 1).min(count - 1);
            }
            Ok(focused_surface_id(state))
        }
        KEY_ENTER => {
            if count == 0 {
                return Ok(focused_surface_id(state));
            }
            let action = results[state.overlay_selection.min(count - 1)];
            state.overlay_mode = OverlayMode::None;
            state.overlay_selection = 0;
            state.palette_query_len = 0;
            perform_palette_action(state, action)
        }
        _ => Ok(focused_surface_id(state)),
    }
}

pub(super) fn handle_notification_overlay_key(
    state: &mut DesktopState,
    key_code: u32,
) -> rt::Result<u32> {
    match key_code {
        KEY_UP => {
            state.overlay_selection = state.overlay_selection.saturating_sub(1);
            Ok(focused_surface_id(state))
        }
        KEY_DOWN => {
            if state.notification_history_len != 0 {
                state.overlay_selection =
                    (state.overlay_selection + 1).min(state.notification_history_len - 1);
            }
            Ok(focused_surface_id(state))
        }
        KEY_ENTER => {
            if let Some(entry) = state
                .notification_history
                .iter()
                .copied()
                .take(state.notification_history_len)
                .nth(state.overlay_selection)
            {
                if entry.actionable {
                    if let Some(app_id) = entry.source_app {
                        state.overlay_mode = OverlayMode::None;
                        state.overlay_selection = 0;
                        return focus_app(state, app_id);
                    }
                }
            }
            Ok(focused_surface_id(state))
        }
        _ => Ok(focused_surface_id(state)),
    }
}

pub(super) fn handle_clipboard_overlay_key(
    state: &mut DesktopState,
    key_code: u32,
) -> rt::Result<u32> {
    match key_code {
        KEY_UP => {
            state.overlay_selection = state.overlay_selection.saturating_sub(1);
            Ok(focused_surface_id(state))
        }
        KEY_DOWN => {
            state.overlay_selection = (state.overlay_selection + 1).min(CLIPBOARD_HISTORY_LINES - 1);
            Ok(focused_surface_id(state))
        }
        KEY_ENTER => {
            if state.clipboard_service_handle == rt::INVALID_HANDLE {
                post_notification(state, None, false, b"clipboard service unavailable")?;
                state.overlay_mode = OverlayMode::None;
                state.overlay_selection = 0;
                return Ok(focused_surface_id(state));
            }
            rt::clipboard_activate(state.clipboard_service_handle, state.overlay_selection as u32)?;
            state.overlay_mode = OverlayMode::None;
            state.overlay_selection = 0;
            post_notification(state, None, false, b"clipboard selection activated")?;
            Ok(focused_surface_id(state))
        }
        _ => Ok(focused_surface_id(state)),
    }
}

fn perform_palette_action(state: &mut DesktopState, action: PaletteAction) -> rt::Result<u32> {
    match action {
        PaletteAction::Launch(app_id) => crate::windows::launch_or_focus_app(state, app_id),
        PaletteAction::ShowNotifications => {
            state.overlay_mode = OverlayMode::Notifications;
            Ok(focused_surface_id(state))
        }
        PaletteAction::ShowClipboardHistory => {
            state.overlay_mode = OverlayMode::ClipboardHistory;
            Ok(focused_surface_id(state))
        }
        PaletteAction::SwitchWorkspace(workspace_id) => switch_workspace(state, workspace_id),
        PaletteAction::FocusNext => focus_next_app(state),
    }
}
