use core::char;

use rt::PermissionPolicyState;
use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;

use crate::netdiag;
use crate::render::render;
use crate::security::{first_actionable_runtime, security_policy_count, update_policy};
use crate::state::*;

pub(crate) enum ControlFlow {
    Continue,
    Exit,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn poll_control(
    control_handle: rt::Handle,
    buffers: &mut ui::SurfaceBuffers<SURFACE_BUFFER_SLOTS>,
    presenter: &mut ui::FirstPresentSurface,
    config_handle: rt::Handle,
    network_handle: rt::Handle,
    audio_handle: rt::Handle,
    runtime_handle: rt::Handle,
    security_handle: rt::Handle,
    audio_stream_handle: rt::Handle,
    state: &mut AppState,
) -> rt::Result<ControlFlow> {
    let mut changed = false;
    loop {
        let mut message = rt::RawMessage::empty(0);
        match rt::channel_receive_nonblocking(control_handle, &mut message) {
            Ok(())
                if message.tag == rt::AppControlTag::FocusChanged as u32
                    && message.word_count > 0 =>
            {
                state.focused = message.words[0] != 0;
                changed = true;
            }
            Ok(())
                if message.tag == rt::AppControlTag::Resize as u32 && message.word_count >= 2 =>
            {
                state.width = message.words[0] as u32;
                state.height = message.words[1] as u32;
                changed = true;
            }
            Ok(())
                if message.tag == rt::AppControlTag::Pointer as u32 && message.word_count >= 4 =>
            {
                let action = ui::decode_app_pointer_action(message.words[0]);
                let x = message.words[1] as i64 as i32;
                let y = message.words[2] as i64 as i32;
                if matches!(action, Some(rt::AppPointerAction::Down)) {
                    changed |= handle_pointer_down(
                        network_handle,
                        runtime_handle,
                        security_handle,
                        audio_stream_handle,
                        state,
                        x,
                        y,
                    )?;
                }
            }
            Ok(()) if message.tag == rt::AppControlTag::Key as u32 && message.word_count >= 2 => {
                if matches!(
                    ui::decode_app_key_action(message.words[0]),
                    Some(rt::AppKeyAction::Down)
                ) {
                    changed |= handle_key_down(
                        network_handle,
                        security_handle,
                        state,
                        message.words[1] as u32,
                    )?;
                }
            }
            Ok(()) if message.tag == rt::AppControlTag::Text as u32 && message.word_count > 0 => {
                changed |= append_text_input(state, message.words[0]);
            }
            Ok(()) if message.tag == rt::AppControlTag::Close as u32 => {
                return Ok(ControlFlow::Exit);
            }
            Ok(()) => {}
            Err(rt::Error::QueueEmpty) => break,
            Err(error) => return Err(error),
        }
    }

    if changed {
        let (slot, buffer) = buffers.advance();
        render(
            presenter,
            slot,
            buffer,
            config_handle,
            network_handle,
            audio_handle,
            runtime_handle,
            security_handle,
            state,
        )?;
    }

    Ok(ControlFlow::Continue)
}

#[allow(clippy::too_many_arguments)]
fn handle_pointer_down(
    network_handle: rt::Handle,
    runtime_handle: rt::Handle,
    security_handle: rt::Handle,
    audio_stream_handle: rt::Handle,
    state: &mut AppState,
    x: i32,
    y: i32,
) -> rt::Result<bool> {
    if x >= TAB_SYSTEM_X0 && x < TAB_SYSTEM_X1 && y >= TAB_Y0 && y < TAB_Y1 {
        state.page = SettingsPage::System;
        state.editing_note = false;
        state.editing_hostname = false;
        return Ok(true);
    }
    if x >= TAB_SECURITY_X0 && x < TAB_SECURITY_X1 && y >= TAB_Y0 && y < TAB_Y1 {
        state.page = SettingsPage::Security;
        state.editing_note = false;
        state.editing_hostname = false;
        return Ok(true);
    }
    if x >= TAB_NETWORK_X0 && x < TAB_NETWORK_X1 && y >= TAB_Y0 && y < TAB_Y1 {
        state.page = SettingsPage::Network;
        state.editing_note = false;
        return Ok(true);
    }

    if state.page == SettingsPage::Network {
        return handle_network_pointer_down(network_handle, state, x, y);
    }

    if state.page == SettingsPage::System {
        if x >= NOTE_FIELD_X0 && x < NOTE_FIELD_X1 && y >= NOTE_FIELD_Y0 && y < NOTE_FIELD_Y1 {
            state.editing_note = true;
            return Ok(true);
        }
        if x >= AUDIO_TEST_X0 && x < AUDIO_TEST_X1 && y >= AUDIO_TEST_Y0 && y < AUDIO_TEST_Y1 {
            if audio_stream_handle != rt::INVALID_HANDLE {
                let _ = rt::audio_stream_play_tone(audio_stream_handle, 880, 120);
            }
            state.editing_note = false;
            return Ok(true);
        }
        state.editing_note = false;
        return Ok(true);
    }

    state.editing_note = false;
    if x >= SEC_PREV_X0 && x < SEC_PREV_X1 && y >= SEC_ACTION_Y0 && y < SEC_ACTION_Y1 {
        if state.selected_policy_index > 0 {
            state.selected_policy_index -= 1;
        }
        return Ok(true);
    }
    if x >= SEC_NEXT_X0 && x < SEC_NEXT_X1 && y >= SEC_ACTION_Y0 && y < SEC_ACTION_Y1 {
        let count = security_policy_count(security_handle)?;
        if state.selected_policy_index + 1 < count {
            state.selected_policy_index += 1;
        }
        return Ok(true);
    }
    if x >= SEC_ALLOW_X0 && x < SEC_ALLOW_X1 && y >= SEC_ACTION_Y0 && y < SEC_ACTION_Y1 {
        update_policy(
            security_handle,
            state.selected_policy_index,
            PermissionPolicyState::Allowed,
        )?;
        return Ok(true);
    }
    if x >= SEC_BLOCK_X0 && x < SEC_BLOCK_X1 && y >= SEC_ACTION_Y0 && y < SEC_ACTION_Y1 {
        update_policy(
            security_handle,
            state.selected_policy_index,
            PermissionPolicyState::Blocked,
        )?;
        return Ok(true);
    }
    if x >= SEC_DEFAULT_X0 && x < SEC_DEFAULT_X1 && y >= SEC_ACTION_Y0 && y < SEC_ACTION_Y1 {
        update_policy(
            security_handle,
            state.selected_policy_index,
            PermissionPolicyState::DefaultAllow,
        )?;
        return Ok(true);
    }
    if x >= SEC_APPROVE_X0 && x < SEC_APPROVE_X1 && y >= SEC_RUNTIME_Y0 && y < SEC_RUNTIME_Y1 {
        if let Some(runtime) = first_actionable_runtime(runtime_handle)? {
            rt::runtime_env_decide(
                runtime_handle,
                runtime.env_id,
                PermissionPolicyState::Allowed,
            )?;
        }
        return Ok(true);
    }
    if x >= SEC_DENY_X0 && x < SEC_DENY_X1 && y >= SEC_RUNTIME_Y0 && y < SEC_RUNTIME_Y1 {
        if let Some(runtime) = first_actionable_runtime(runtime_handle)? {
            rt::runtime_env_decide(
                runtime_handle,
                runtime.env_id,
                PermissionPolicyState::Blocked,
            )?;
        }
        return Ok(true);
    }
    if x >= SEC_RESET_X0 && x < SEC_RESET_X1 && y >= SEC_RUNTIME_Y0 && y < SEC_RUNTIME_Y1 {
        if let Some(runtime) = first_actionable_runtime(runtime_handle)? {
            rt::runtime_env_decide(
                runtime_handle,
                runtime.env_id,
                PermissionPolicyState::DefaultAllow,
            )?;
        }
        return Ok(true);
    }
    Ok(true)
}

/// Feed one text event into the active editor (note field on System,
/// hostname field on Network). Returns whether the frame changed.
fn append_text_input(state: &mut AppState, word: u64) -> bool {
    let Some(ch) = char::from_u32(word as u32) else {
        return false;
    };
    if state.editing_note {
        if ch == '\n' {
            state.editing_note = false;
            return true;
        }
        if ch.is_ascii_graphic() || ch == ' ' {
            let mut scratch = [0u8; 4];
            let bytes = ch.encode_utf8(&mut scratch).as_bytes();
            if state.note_len + bytes.len() <= NOTE_MAX_BYTES {
                state.note[state.note_len..state.note_len + bytes.len()].copy_from_slice(bytes);
                state.note_len += bytes.len();
                return true;
            }
        }
        return false;
    }
    if state.editing_hostname && ch.is_ascii_graphic() && ch != ' ' {
        if state.hostname_edit_len < HOSTNAME_EDIT_MAX_BYTES {
            state.hostname_edit[state.hostname_edit_len] = ch as u8;
            state.hostname_edit_len += 1;
            return true;
        }
    }
    false
}

fn handle_network_pointer_down(
    network_handle: rt::Handle,
    state: &mut AppState,
    x: i32,
    y: i32,
) -> rt::Result<bool> {
    if x >= NET_HOSTNAME_FIELD_X0
        && x < NET_HOSTNAME_FIELD_X1
        && y >= NET_HOSTNAME_FIELD_Y0
        && y < NET_HOSTNAME_FIELD_Y1
    {
        if !state.editing_hostname {
            state.hostname_edit = [0; HOSTNAME_EDIT_MAX_BYTES];
            state.hostname_edit_len = 0;
            state.editing_hostname = true;
        }
        return Ok(true);
    }
    if x >= NET_PING_RUN_X0 && x < NET_PING_RUN_X1 && y >= NET_PING_RUN_Y0 && y < NET_PING_RUN_Y1 {
        let target = netdiag::ping_target_address(
            rt::network_interface_status(network_handle, 0).unwrap_or(None),
        );
        state.editing_hostname = false;
        match target {
            Some(target) => netdiag::run_ping(network_handle, target, state),
            None => {
                state.ping_stats = None;
                state.ping_failed = true;
                state.ping_target_len = 0;
            }
        }
        return Ok(true);
    }
    state.editing_hostname = false;
    Ok(true)
}

fn handle_key_down(
    network_handle: rt::Handle,
    security_handle: rt::Handle,
    state: &mut AppState,
    key: u32,
) -> rt::Result<bool> {
    match key {
        14 if state.editing_note && state.note_len > 0 => {
            state.note_len -= 1;
            Ok(true)
        }
        14 if state.editing_hostname && state.hostname_edit_len > 0 => {
            state.hostname_edit_len -= 1;
            Ok(true)
        }
        28 if state.editing_hostname => {
            netdiag::commit_hostname(network_handle, state);
            state.editing_hostname = false;
            Ok(true)
        }
        1 if state.editing_hostname => {
            state.editing_hostname = false;
            Ok(true)
        }
        15 => {
            state.page = state.page.next();
            state.editing_note = false;
            state.editing_hostname = false;
            Ok(true)
        }
        103 if state.page == SettingsPage::Security => {
            if state.selected_policy_index > 0 {
                state.selected_policy_index -= 1;
                return Ok(true);
            }
            Ok(false)
        }
        108 if state.page == SettingsPage::Security => {
            let count = security_policy_count(security_handle)?;
            if state.selected_policy_index + 1 < count {
                state.selected_policy_index += 1;
                return Ok(true);
            }
            Ok(false)
        }
        _ => Ok(false),
    }
}

pub(crate) fn cleanup_audio(audio_stream_handle: rt::Handle, audio_handle: rt::Handle) {
    if audio_stream_handle != rt::INVALID_HANDLE {
        let _ = rt::audio_stream_close(audio_stream_handle);
        let _ = rt::handle_close(audio_stream_handle);
    }
    if audio_handle != rt::INVALID_HANDLE {
        let _ = rt::handle_close(audio_handle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_state() -> AppState {
        AppState {
            width: 320,
            height: 300,
            focused: true,
            page: SettingsPage::System,
            editing_note: false,
            editing_hostname: false,
            selected_policy_index: 0,
            note: [0; NOTE_MAX_BYTES],
            note_len: 0,
            hostname_edit: [0; HOSTNAME_EDIT_MAX_BYTES],
            hostname_edit_len: 0,
            ping_stats: None,
            ping_failed: false,
            ping_target: [0; PING_TARGET_MAX_BYTES],
            ping_target_len: 0,
        }
    }

    #[test]
    fn tab_key_cycles_all_three_pages() {
        let mut state = app_state();
        assert_eq!(state.page, SettingsPage::System);
        let _ = handle_key_down(rt::INVALID_HANDLE, rt::INVALID_HANDLE, &mut state, 15);
        assert_eq!(state.page, SettingsPage::Security);
        let _ = handle_key_down(rt::INVALID_HANDLE, rt::INVALID_HANDLE, &mut state, 15);
        assert_eq!(state.page, SettingsPage::Network);
        let _ = handle_key_down(rt::INVALID_HANDLE, rt::INVALID_HANDLE, &mut state, 15);
        assert_eq!(state.page, SettingsPage::System);
    }

    #[test]
    fn tab_key_leaves_editing_state() {
        let mut state = app_state();
        state.page = SettingsPage::Network;
        state.editing_hostname = true;
        let _ = handle_key_down(rt::INVALID_HANDLE, rt::INVALID_HANDLE, &mut state, 15);
        assert!(!state.editing_hostname);
        assert_eq!(state.page, SettingsPage::System);
    }

    #[test]
    fn hostname_typing_backspace_and_commit_via_invalid_handle() {
        let mut state = app_state();
        state.page = SettingsPage::Network;
        state.editing_hostname = true;
        for ch in b"gw-x" {
            let mut message = rt::RawMessage::empty(rt::AppControlTag::Text as u32);
            message.word_count = 1;
            message.words[0] = *ch as u64;
            let _ = append_text_input(&mut state, message.words[0]);
        }
        assert_eq!(state.hostname_edit_len, 4);
        assert_eq!(&state.hostname_edit[..4], b"gw-x");

        // Backspace then retype.
        let _ = handle_key_down(rt::INVALID_HANDLE, rt::INVALID_HANDLE, &mut state, 14);
        assert_eq!(state.hostname_edit_len, 3);
        let _ = append_text_input(&mut state, b'9' as u64);
        assert_eq!(&state.hostname_edit[..4], b"gw-9");

        // Enter commits via the runtime wrapper; transport on INVALID_HANDLE
        // fails so commit_hostname returns false but must not panic.
        let _ = handle_key_down(rt::INVALID_HANDLE, rt::INVALID_HANDLE, &mut state, 28);
        assert!(!state.editing_hostname);

        // Esc cancels an edit without committing.
        state.editing_hostname = true;
        let _ = handle_key_down(rt::INVALID_HANDLE, rt::INVALID_HANDLE, &mut state, 1);
        assert!(!state.editing_hostname);
    }

    #[test]
    fn empty_hostname_commit_is_rejected_without_transport() {
        let mut state = app_state();
        state.page = SettingsPage::Network;
        state.hostname_edit_len = 0;
        assert!(!netdiag::commit_hostname(rt::INVALID_HANDLE, &mut state));
    }

    #[test]
    fn network_tab_pointer_focuses_hostname_field() {
        let mut state = app_state();
        state.page = SettingsPage::Network;
        let changed = handle_network_pointer_down(
            rt::INVALID_HANDLE,
            &mut state,
            NET_HOSTNAME_FIELD_X0 + 4,
            NET_HOSTNAME_FIELD_Y0 + 4,
        )
        .expect("pointer handled");
        assert!(changed);
        assert!(state.editing_hostname);

        // Second click inside the field keeps the typed text.
        state.hostname_edit_len = 2;
        let _ = handle_network_pointer_down(
            rt::INVALID_HANDLE,
            &mut state,
            NET_HOSTNAME_FIELD_X0 + 4,
            NET_HOSTNAME_FIELD_Y0 + 4,
        );
        assert_eq!(state.hostname_edit_len, 2);
    }

    #[test]
    fn ping_button_without_service_degrades_to_failed() {
        let mut state = app_state();
        state.page = SettingsPage::Network;
        let changed = handle_network_pointer_down(
            rt::INVALID_HANDLE,
            &mut state,
            NET_PING_RUN_X0 + 4,
            NET_PING_RUN_Y0 + 4,
        )
        .expect("pointer handled");
        assert!(changed);
        assert!(state.ping_failed);
        assert!(state.ping_stats.is_none());

        // Clicking elsewhere on the page only closes the editor.
        state.editing_hostname = true;
        let _ = handle_network_pointer_down(rt::INVALID_HANDLE, &mut state, 4, 4);
        assert!(!state.editing_hostname);
    }
}
