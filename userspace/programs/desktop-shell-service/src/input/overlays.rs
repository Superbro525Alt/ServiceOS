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
        .and_then(|app_id| {
            candidates[..count]
                .iter()
                .position(|candidate| *candidate == app_id)
        })
        .unwrap_or(0);
    let next = candidates[(current_index + offset) % count];
    focus_app(state, next)
}

pub(crate) fn clamp_clipboard_selection(selection: usize) -> usize {
    selection.min(CLIPBOARD_HISTORY_LINES - 1)
}

fn selected_notification_entry(state: &DesktopState) -> Option<crate::NotificationEntry> {
    let limit = state
        .notification_history_len
        .min(crate::NOTIFICATION_HISTORY_MAX);
    let index = state.overlay_selection.min(limit.saturating_sub(1));
    state
        .notification_history
        .get(index)
        .copied()
        .filter(|entry| entry.occupied)
}

pub(super) fn focus_notification_source(state: &mut DesktopState) -> rt::Result<u32> {
    if let Some(entry) = selected_notification_entry(state) {
        if let Some(app_id) = entry.source_app {
            state.overlay_mode = OverlayMode::None;
            state.overlay_selection = 0;
            return focus_app(state, app_id);
        }
    }
    Ok(focused_surface_id(state))
}

pub(super) fn dismiss_all_notifications_now(state: &mut DesktopState) -> rt::Result<()> {
    crate::windows::dismiss_all_notifications(state);
    render_overlays_only(state)
}

pub(crate) fn paste_clipboard_selection(state: &mut DesktopState, row: usize) -> rt::Result<u32> {
    if state.clipboard_service_handle == rt::INVALID_HANDLE {
        post_notification(state, None, false, b"clipboard service unavailable")?;
        state.overlay_mode = OverlayMode::None;
        state.overlay_selection = 0;
        return Ok(focused_surface_id(state));
    }
    let index = clamp_clipboard_selection(row);
    rt::clipboard_activate(state.clipboard_service_handle, index as u32)?;
    state.overlay_mode = OverlayMode::None;
    state.overlay_selection = 0;
    post_notification(state, None, false, b"clipboard selection activated")?;

    if let Some(app_id) = state.focused_app {
        if let Some(slot) = app_slot_index(&state.apps, app_id) {
            let control = state.apps[slot].window.control_handle;
            if control != rt::INVALID_HANDLE {
                rt::app_control_key(control, AppKeyAction::Down, KEY_V, MOD_CTRL)?;
                rt::app_control_key(control, AppKeyAction::Up, KEY_V, MOD_CTRL)?;
            }
        }
    }
    Ok(focused_surface_id(state))
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
        KEY_A => {
            dismiss_all_notifications_now(state)?;
            Ok(focused_surface_id(state))
        }
        KEY_F => focus_notification_source(state),
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
            if let Some(entry) = selected_notification_entry(state) {
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
            state.overlay_selection =
                (state.overlay_selection + 1).min(CLIPBOARD_HISTORY_LINES - 1);
            Ok(focused_surface_id(state))
        }
        KEY_ENTER => paste_clipboard_selection(state, state.overlay_selection),
        _ => Ok(focused_surface_id(state)),
    }
}

pub(super) fn handle_media_overlay_key(state: &mut DesktopState, key_code: u32) -> rt::Result<u32> {
    match key_code {
        KEY_UP | KEY_DOWN => {
            let delta = if key_code == KEY_UP {
                crate::media::MEDIA_VOLUME_STEP
            } else {
                -crate::media::MEDIA_VOLUME_STEP
            };
            let next = crate::media::step_volume(state.master_volume, delta);
            match crate::media::request_master_volume(
                state.audio_service_handle,
                next,
                state.master_muted,
            ) {
                Ok((applied_volume, applied_muted)) => {
                    state.master_volume = applied_volume;
                    state.master_muted = applied_muted;
                }
                Err(_) => {
                    post_notification(state, None, false, b"audio service rejected volume change")?;
                }
            }
            state.pending_media_refresh.set();
            Ok(focused_surface_id(state))
        }
        KEY_SPACE => {
            let muted = !state.master_muted;
            match crate::media::request_master_volume(
                state.audio_service_handle,
                state.master_volume,
                muted,
            ) {
                Ok((applied_volume, applied_muted)) => {
                    state.master_volume = applied_volume;
                    state.master_muted = applied_muted;
                }
                Err(_) => {
                    post_notification(state, None, false, b"audio service rejected mute change")?;
                }
            }
            state.pending_media_refresh.set();
            Ok(focused_surface_id(state))
        }
        _ => Ok(focused_surface_id(state)),
    }
}

fn perform_palette_action(state: &mut DesktopState, action: PaletteAction) -> rt::Result<u32> {
    match action {
        PaletteAction::Launch(app_id) => {
            crate::windows::schedule_launch_or_focus_app(state, app_id)
        }
        PaletteAction::ShowNotifications => {
            state.overlay_mode = OverlayMode::Notifications;
            Ok(focused_surface_id(state))
        }
        PaletteAction::ShowClipboardHistory => {
            state.overlay_mode = OverlayMode::ClipboardHistory;
            Ok(focused_surface_id(state))
        }
        PaletteAction::ShowMedia | PaletteAction::ToggleMedia => {
            state.overlay_mode = if state.overlay_mode == OverlayMode::Media {
                OverlayMode::None
            } else {
                OverlayMode::Media
            };
            state.overlay_selection = 0;
            Ok(focused_surface_id(state))
        }
        PaletteAction::ToggleNotifications => {
            state.overlay_mode = if state.overlay_mode == OverlayMode::Notifications {
                OverlayMode::None
            } else {
                OverlayMode::Notifications
            };
            state.overlay_selection = 0;
            Ok(focused_surface_id(state))
        }
        PaletteAction::ToggleClipboardHistory => {
            state.overlay_mode = if state.overlay_mode == OverlayMode::ClipboardHistory {
                OverlayMode::None
            } else {
                OverlayMode::ClipboardHistory
            };
            state.overlay_selection = 0;
            Ok(focused_surface_id(state))
        }
        PaletteAction::DismissAllNotifications => {
            dismiss_all_notifications_now(state)?;
            Ok(focused_surface_id(state))
        }
        PaletteAction::CycleSettingsPage => cycle_settings_page(state),
        PaletteAction::LockSession => lock_session_stub(state),
        PaletteAction::SwitchWorkspace(workspace_id) => switch_workspace(state, workspace_id),
        PaletteAction::FocusNext => focus_next_app(state),
    }
}

fn cycle_settings_page(state: &mut DesktopState) -> rt::Result<u32> {
    let index = app_slot_index(&state.apps, DesktopAppId::Settings).ok_or(rt::Error::NotFound)?;
    if state.apps[index].running && state.apps[index].window.control_handle != rt::INVALID_HANDLE {
        let control = state.apps[index].window.control_handle;
        rt::app_control_key(control, AppKeyAction::Down, KEY_TAB, 0)?;
        rt::app_control_key(control, AppKeyAction::Up, KEY_TAB, 0)?;
        return focus_app(state, DesktopAppId::Settings);
    }
    crate::windows::schedule_launch_or_focus_app(state, DesktopAppId::Settings)
}

fn lock_session_stub(state: &mut DesktopState) -> rt::Result<u32> {
    state.overlay_mode = OverlayMode::None;
    state.overlay_selection = 0;
    state.switcher_selection = 0;
    state.palette_query_len = 0;
    post_notification(state, None, false, b"SESSION LOCKED (SHELL STUB)")?;
    Ok(focused_surface_id(state))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_selection_clamps_to_panel_rows() {
        assert_eq!(clamp_clipboard_selection(0), 0);
        assert_eq!(
            clamp_clipboard_selection(CLIPBOARD_HISTORY_LINES - 1),
            CLIPBOARD_HISTORY_LINES - 1
        );
        assert_eq!(
            clamp_clipboard_selection(usize::MAX),
            CLIPBOARD_HISTORY_LINES - 1
        );
    }

    #[test]
    fn clipboard_rows_map_identity_into_ring_order() {
        for row in 0..CLIPBOARD_HISTORY_LINES {
            assert_eq!(clamp_clipboard_selection(row), row);
        }
    }

    #[test]
    fn notification_quick_action_keys_are_recognized_scancodes() {
        assert_eq!(KEY_A, 30);
        assert_eq!(KEY_F, 33);
        assert_ne!(KEY_A, KEY_F);
    }
}
