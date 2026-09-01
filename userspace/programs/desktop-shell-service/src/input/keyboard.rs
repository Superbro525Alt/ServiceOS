use super::*;

pub(super) fn commit_switcher(state: &mut DesktopState) -> rt::Result<u32> {
    let model = crate::switcher::switcher_model(state);
    let target = model.target(state.switcher_selection);
    state.overlay_mode = OverlayMode::None;
    state.switcher_selection = 0;
    state.overlay_selection = 0;
    match target {
        Some(app_id) => focus_app(state, app_id),
        None => Ok(focused_surface_id(state)),
    }
}

fn handle_switcher_key(state: &mut DesktopState, key_code: u32, modifiers: u32) -> rt::Result<u32> {
    let model = crate::switcher::switcher_model(state);
    if key_code != KEY_TAB || model.count == 0 {
        return Ok(focused_surface_id(state));
    }
    state.switcher_selection = crate::switcher::advance_selection(
        model.count,
        state.switcher_selection,
        modifiers & MOD_SHIFT == 0,
    );
    Ok(focused_surface_id(state))
}

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
        return commit_switcher(state);
    }

    if action == AppKeyAction::Down {
        // Launcher document-row keyboard focus (palette-shaped flow): armed
        // only while no overlay is up, so overlay keys are untouched, and
        // any overlay opening disarms it. Ctrl+Tab is the entry chord — it
        // stays outside the global action registry on purpose.
        if state.overlay_mode != OverlayMode::None {
            state.launcher_doc_focus = None;
        } else if modifiers == MOD_CTRL && key_code == KEY_TAB {
            return crate::launcher_docs::begin_doc_focus(state);
        } else if state.launcher_doc_focus.is_some() {
            return crate::launcher_docs::handle_doc_focus_key(state, key_code);
        }

        if key_code == KEY_ESC && state.overlay_mode != OverlayMode::None {
            if state.overlay_mode == OverlayMode::Login {
                crate::login::reset_login(state);
            }
            if state.overlay_mode == OverlayMode::Approval {
                crate::approvals::note_overlay_closed(&mut state.approvals);
            }
            state.overlay_mode = OverlayMode::None;
            state.overlay_selection = 0;
            state.switcher_selection = 0;
            state.palette_query_len = 0;
            crate::palette_docs::clear_doc_hits(state);
            return Ok(focused_surface_id(state));
        }

        // Global shell shortcuts resolve through the shared action registry —
        // the same single source that backs palette entries, hot corners,
        // and notification quick actions.
        if let Some(shell_action) = crate::actions::action_for_binding(modifiers, key_code) {
            return crate::actions::execute_shell_action(state, shell_action);
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
        if state.overlay_mode == OverlayMode::Media {
            return overlays::handle_media_overlay_key(state, key_code);
        }
        if state.overlay_mode == OverlayMode::WorkspaceOverview {
            return overlays::handle_workspace_overview_key(state, key_code);
        }
        if state.overlay_mode == OverlayMode::Login {
            return crate::login::handle_login_key(state, key_code, modifiers);
        }
        if state.overlay_mode == OverlayMode::Approval {
            return overlays::handle_approval_overlay_key(state, key_code);
        }
    }

    if action == AppKeyAction::Down && modifiers & MOD_ALT != 0 {
        if key_code == KEY_TAB {
            if state.overlay_mode != OverlayMode::Switcher {
                let model = crate::switcher::switcher_model(state);
                state.overlay_mode = OverlayMode::Switcher;
                state.switcher_selection =
                    crate::switcher::open_selection(&model, state.focused_app);
                return Ok(focused_surface_id(state));
            }
            return handle_switcher_key(state, key_code, modifiers);
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
    Ok(state.apps[index].window.surface_id)
}

pub(super) fn handle_text_input(state: &mut DesktopState, scalar: u32) -> rt::Result<u32> {
    let Some(ch) = core::char::from_u32(scalar) else {
        return Ok(focused_surface_id(state));
    };
    if state.overlay_mode == OverlayMode::Login {
        // Login fields are fed through the scancode path only; unicode text
        // events must not leak into focused apps behind the overlay.
        return Ok(focused_surface_id(state));
    }
    if state.overlay_mode == OverlayMode::CommandPalette {
        if !ch.is_control() && state.palette_query_len < state.palette_query.len() {
            state.palette_query[state.palette_query_len] = ch as u8;
            state.palette_query_len += 1;
            state.overlay_selection = 0;
            crate::palette_docs::refresh_doc_hits(state);
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
