use core::char;

use rt::PermissionPolicyState;
use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;

use crate::netdiag;
use crate::render::render;
use crate::security::{
    POLICY_KINDS, first_actionable_runtime, next_policy_default, policy_get, policy_set,
    prev_policy_default, security_policy_count, update_policy,
};
use crate::state::*;
use crate::wifi;

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
    backup_handle: rt::Handle,
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
                        backup_handle,
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
                        backup_handle,
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
    backup_handle: rt::Handle,
    audio_stream_handle: rt::Handle,
    state: &mut AppState,
    x: i32,
    y: i32,
) -> rt::Result<bool> {
    if x >= TAB_SYSTEM_X0 && x < TAB_SYSTEM_X1 && y >= TAB_Y0 && y < TAB_Y1 {
        state.page = SettingsPage::System;
        state.editing_note = false;
        state.editing_hostname = false;
        state.wifi.stop_editing();
        return Ok(true);
    }
    if x >= TAB_SECURITY_X0 && x < TAB_SECURITY_X1 && y >= TAB_Y0 && y < TAB_Y1 {
        state.page = SettingsPage::Security;
        state.editing_note = false;
        state.editing_hostname = false;
        state.wifi.stop_editing();
        return Ok(true);
    }
    if x >= TAB_NETWORK_X0 && x < TAB_NETWORK_X1 && y >= TAB_Y0 && y < TAB_Y1 {
        state.page = SettingsPage::Network;
        state.editing_note = false;
        state.wifi.stop_editing();
        return Ok(true);
    }
    if x >= TAB_WIFI_X0 && x < TAB_WIFI_X1 && y >= TAB_Y0 && y < TAB_Y1 {
        state.page = SettingsPage::Wifi;
        state.editing_note = false;
        state.editing_hostname = false;
        refresh_saved(network_handle, state);
        return Ok(true);
    }
    if x >= TAB_BACKUP_X0 && x < TAB_BACKUP_X1 && y >= TAB_Y0 && y < TAB_Y1 {
        state.page = SettingsPage::Backup;
        state.editing_note = false;
        state.editing_hostname = false;
        state.wifi.stop_editing();
        crate::backup::on_page_enter(backup_handle, &mut state.backup);
        return Ok(true);
    }

    if state.page == SettingsPage::Wifi {
        return handle_wifi_pointer_down(network_handle, state, x, y);
    }

    if state.page == SettingsPage::Backup {
        state.editing_note = false;
        return handle_backup_pointer_down(backup_handle, state, x, y);
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
    if let Some(step) = env_policy_step(x, y) {
        let kind = POLICY_KINDS[step.row];
        // The selector cycles from the service's current default so the
        // button always means "one step" from what the page shows, and a
        // failed read never writes a blind value.
        if let Ok(current) = policy_get(runtime_handle, kind) {
            let next = match step.direction {
                PolicyStepDirection::Next => next_policy_default(current),
                PolicyStepDirection::Prev => prev_policy_default(current),
            };
            let _ = policy_set(runtime_handle, kind, next);
        }
        return Ok(true);
    }
    Ok(true)
}

/// Pointer direction + row for the per-kind policy selector buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PolicyStepDirection {
    Prev,
    Next,
}

struct PolicyStep {
    row: usize,
    direction: PolicyStepDirection,
}

/// Hitbox decode for the ENV POLICY rows: `<`/`>` buttons per kind row.
/// Pure so host tests cover the geometry without handles.
fn env_policy_step(x: i32, y: i32) -> Option<PolicyStep> {
    let rows = [(SEC_POLICY_ROW0_Y0, 0), (SEC_POLICY_ROW1_Y0, 1)];
    for (y0, row) in rows {
        if y < y0 || y >= y0 + SEC_POLICY_ROW_H {
            continue;
        }
        let direction = if x >= SEC_POLICY_PREV_X0 && x < SEC_POLICY_PREV_X1 {
            PolicyStepDirection::Prev
        } else if x >= SEC_POLICY_NEXT_X0 && x < SEC_POLICY_NEXT_X1 {
            PolicyStepDirection::Next
        } else {
            continue;
        };
        return Some(PolicyStep { row, direction });
    }
    None
}

/// Feed one text event into the active editor (note field on System,
/// hostname field on Network, modal prompt on Wifi). Returns whether the
/// frame changed.
fn append_text_input(state: &mut AppState, word: u64) -> bool {
    let Some(ch) = char::from_u32(word as u32) else {
        return false;
    };
    if state.wifi.prompt.is_some() {
        return wifi::wifi_prompt_char(&mut state.wifi, ch);
    }
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

/// Load the saved-network list into page state. Failures (Unsupported
/// without a backend) leave the list empty and surface in `saved_total`
/// semantics: count stays 0 and the page shows the honest unavailable line.
/// Pointer routing on the Backup page. Without a trusted route the page is
/// the manual-activation explainer and every click is inert. With a route:
/// prompts are modal (confirm/cancel only), then the action buttons, then
/// the snapshot rows for selection.
fn handle_backup_pointer_down(
    backup_handle: rt::Handle,
    state: &mut AppState,
    x: i32,
    y: i32,
) -> rt::Result<bool> {
    if !crate::backup::page_live(&state.backup) {
        return Ok(false);
    }

    if state.backup.prompt.is_some() {
        if x >= BACKUP_CONFIRM_BTN_X0
            && x < BACKUP_CONFIRM_BTN_X1
            && y >= BACKUP_PROMPT_BTN_Y0
            && y < BACKUP_PROMPT_BTN_Y1
        {
            match state.backup.prompt {
                Some(crate::backup::BackupPrompt::RestoreConfirm(_)) => {
                    crate::backup::perform_restore_apply(
                        backup_handle,
                        &mut state.backup,
                        crate::backup::BACKUP_SCOPE_KNOWN_MASK,
                    );
                }
                Some(crate::backup::BackupPrompt::DeleteConfirm) => {
                    crate::backup::perform_delete(backup_handle, &mut state.backup);
                }
                None => {}
            }
            return Ok(true);
        }
        if x >= BACKUP_CANCEL_BTN_X0
            && x < BACKUP_CANCEL_BTN_X1
            && y >= BACKUP_PROMPT_BTN_Y0
            && y < BACKUP_PROMPT_BTN_Y1
        {
            state.backup.cancel_prompt();
            return Ok(true);
        }
        // Modal: clicks outside the prompt buttons change nothing.
        return Ok(false);
    }

    if x >= BACKUP_EXPORT_BTN_X0
        && x < BACKUP_EXPORT_BTN_X1
        && y >= BACKUP_BTN_Y0
        && y < BACKUP_BTN_Y1
    {
        crate::backup::perform_export(
            backup_handle,
            &mut state.backup,
            crate::backup::BACKUP_SCOPE_KNOWN_MASK,
        );
        return Ok(true);
    }
    if x >= BACKUP_RESTORE_BTN_X0
        && x < BACKUP_RESTORE_BTN_X1
        && y >= BACKUP_BTN_Y0
        && y < BACKUP_BTN_Y1
    {
        crate::backup::perform_restore_dry_run(
            backup_handle,
            &mut state.backup,
            crate::backup::BACKUP_SCOPE_KNOWN_MASK,
        );
        return Ok(true);
    }
    if x >= BACKUP_DELETE_BTN_X0
        && x < BACKUP_DELETE_BTN_X1
        && y >= BACKUP_BTN_Y0
        && y < BACKUP_BTN_Y1
    {
        let _ = state.backup.begin_delete();
        return Ok(true);
    }

    if x >= WIFI_ROW_X0 && x < WIFI_ROW_X1 && y >= BACKUP_LIST_Y0 {
        let row = ((y - BACKUP_LIST_Y0) / BACKUP_ROW_H) as usize;
        if row < state.backup.entry_count {
            state.backup.select(row);
            return Ok(true);
        }
    }

    Ok(false)
}

fn refresh_saved(network_handle: rt::Handle, state: &mut AppState) {
    match wifi::run_saved_list(network_handle, &mut state.wifi.saved) {
        Ok(total) => {
            state.wifi.saved_count = state.wifi.saved.len();
            state.wifi.saved_total = total;
        }
        Err(_) => {
            state.wifi.saved_count = 0;
            state.wifi.saved_total = 0;
        }
    }
    if state.wifi.selected_saved >= state.wifi.saved_count {
        state.wifi.selected_saved = state.wifi.saved_count.saturating_sub(1);
    }
}

/// Pointer routing on the Wi-Fi page. While the psk/saved-add prompt is
/// open it is modal: clicks inside keep it, clicks outside cancel it, and
/// no button underneath fires.
fn handle_wifi_pointer_down(
    network_handle: rt::Handle,
    state: &mut AppState,
    x: i32,
    y: i32,
) -> rt::Result<bool> {
    if state.wifi.prompt.is_some() {
        let inside = x >= WIFI_ROW_X0
            && x < WIFI_ROW_X1
            && y >= WIFI_SCAN_ROW_Y0.saturating_sub(10)
            && y < WIFI_ACTION_Y0;
        if !inside {
            state.wifi.stop_editing();
        }
        return Ok(true);
    }

    if x >= WIFI_SCAN_BTN_X0 && x < WIFI_SCAN_BTN_X1 && y >= WIFI_BTN_Y0 && y < WIFI_BTN_Y1 {
        wifi::run_scan(network_handle, &mut state.wifi);
        return Ok(true);
    }
    if x >= WIFI_JOIN_BTN_X0 && x < WIFI_JOIN_BTN_X1 && y >= WIFI_BTN_Y0 && y < WIFI_BTN_Y1 {
        wifi::begin_join(network_handle, &mut state.wifi);
        return Ok(true);
    }
    for index in 0..state.wifi.scan_count {
        let y0 = WIFI_SCAN_ROW_Y0 + (index as i32) * WIFI_ROW_H;
        if x >= WIFI_ROW_X0 && x < WIFI_ROW_X1 && y >= y0 && y < y0 + WIFI_ROW_H {
            state.wifi.selected_scan = index;
            return Ok(true);
        }
    }
    for index in 0..state.wifi.saved_count {
        let y0 = WIFI_SAVED_ROW_Y0 + (index as i32) * WIFI_ROW_H;
        if x >= WIFI_ROW_X0 && x < WIFI_ROW_X1 && y >= y0 && y < y0 + WIFI_ROW_H {
            state.wifi.selected_saved = index;
            return Ok(true);
        }
    }
    if x >= WIFI_ADD_BTN_X0 && x < WIFI_ADD_BTN_X1 && y >= WIFI_ACTION_Y0 && y < WIFI_ACTION_Y1 {
        state.wifi.prompt_len = 0;
        state.wifi.prompt_edit = [0; WIFI_EDIT_MAX_BYTES];
        state.wifi.prompt = Some(WifiPrompt::SavedSsid);
        return Ok(true);
    }
    if x >= WIFI_REMOVE_BTN_X0
        && x < WIFI_REMOVE_BTN_X1
        && y >= WIFI_ACTION_Y0
        && y < WIFI_ACTION_Y1
    {
        if state.wifi.saved_count == 0 {
            return Ok(true);
        }
        let selected = state.wifi.selected_saved.min(state.wifi.saved_count - 1);
        let record = state.wifi.saved[selected];
        state.wifi.saved_remove_outcome = Some(
            rt::network_wifi_saved_remove(
                network_handle,
                wifi::ssid_str(&record.ssid, record.ssid_len),
            )
            .map_err(crate::wifi::classify),
        );
        refresh_saved(network_handle, state);
        return Ok(true);
    }
    Ok(true)
}

fn handle_key_down(
    network_handle: rt::Handle,
    security_handle: rt::Handle,
    backup_handle: rt::Handle,
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
            state.wifi.stop_editing();
            if state.page == SettingsPage::Backup {
                crate::backup::on_page_enter(backup_handle, &mut state.backup);
            } else {
                state.backup.stop_editing();
            }
            Ok(true)
        }
        14 if state.wifi.prompt.is_some() => Ok(wifi::wifi_prompt_backspace(&mut state.wifi)),
        28 if state.wifi.prompt.is_some() => {
            let changed = wifi::wifi_prompt_enter(network_handle, &mut state.wifi);
            if changed && state.wifi.prompt.is_none() && state.wifi.saved_add_outcome.is_some() {
                refresh_saved(network_handle, state);
            }
            Ok(changed)
        }
        1 if state.wifi.prompt.is_some() => {
            state.wifi.stop_editing();
            Ok(true)
        }
        28 if state.page == SettingsPage::Wifi => {
            Ok(wifi::begin_join(network_handle, &mut state.wifi))
        }
        103 if state.page == SettingsPage::Wifi => {
            if state.wifi.selected_scan > 0 {
                state.wifi.selected_scan -= 1;
                return Ok(true);
            }
            Ok(false)
        }
        108 if state.page == SettingsPage::Wifi => {
            if state.wifi.selected_scan + 1 < state.wifi.scan_count {
                state.wifi.selected_scan += 1;
                return Ok(true);
            }
            Ok(false)
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
            wifi: WifiUiState::new(),
            backup: crate::backup::BackupUiState::new(),
        }
    }

    #[test]
    fn tab_key_cycles_all_five_pages() {
        let mut state = app_state();
        assert_eq!(state.page, SettingsPage::System);
        let _ = handle_key_down(
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            &mut state,
            15,
        );
        assert_eq!(state.page, SettingsPage::Security);
        let _ = handle_key_down(
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            &mut state,
            15,
        );
        assert_eq!(state.page, SettingsPage::Network);
        let _ = handle_key_down(
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            &mut state,
            15,
        );
        assert_eq!(state.page, SettingsPage::Wifi);
        let _ = handle_key_down(
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            &mut state,
            15,
        );
        assert_eq!(state.page, SettingsPage::Backup);
        let _ = handle_key_down(
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            &mut state,
            15,
        );
        assert_eq!(state.page, SettingsPage::System);
    }

    #[test]
    fn tab_key_leaves_editing_state() {
        let mut state = app_state();
        state.page = SettingsPage::Network;
        state.editing_hostname = true;
        let _ = handle_key_down(
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            &mut state,
            15,
        );
        assert!(!state.editing_hostname);
        assert_eq!(state.page, SettingsPage::Wifi);
        state.editing_hostname = true;
        let _ = handle_key_down(
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            &mut state,
            15,
        );
        assert!(!state.editing_hostname);
        assert_eq!(state.page, SettingsPage::Backup);
        state.editing_hostname = true;
        let _ = handle_key_down(
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            &mut state,
            15,
        );
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
        let _ = handle_key_down(
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            &mut state,
            14,
        );
        assert_eq!(state.hostname_edit_len, 3);
        let _ = append_text_input(&mut state, b'9' as u64);
        assert_eq!(&state.hostname_edit[..4], b"gw-9");

        // Enter commits via the runtime wrapper; transport on INVALID_HANDLE
        // fails so commit_hostname returns false but must not panic.
        let _ = handle_key_down(
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            &mut state,
            28,
        );
        assert!(!state.editing_hostname);

        // Esc cancels an edit without committing.
        state.editing_hostname = true;
        let _ = handle_key_down(
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            &mut state,
            1,
        );
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

    fn wifi_state() -> AppState {
        let mut state = app_state();
        state.page = SettingsPage::Wifi;
        state
    }

    fn secured_scan() -> rt::NetworkWifiScanEntry {
        let mut entry = rt::NetworkWifiScanEntry {
            bssid: [9; 6],
            channel: 11,
            rssi: -40,
            ssid_len: 4,
            ssid: [0; WIFI_SSID_MAX_BYTES],
            security: rt::WifiSecurity::Wpa2,
        };
        entry.ssid[..4].copy_from_slice(b"cafe");
        entry
    }

    #[test]
    fn wifi_tab_pointer_enters_page_and_loads_saved_list() {
        let mut state = wifi_state();
        let changed = handle_pointer_down(
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            &mut state,
            TAB_WIFI_X0 + 4,
            TAB_Y0 + 4,
        )
        .expect("pointer handled");
        assert!(changed);
        assert_eq!(state.page, SettingsPage::Wifi);
        // Saved list loads through the wrapper; without a backend the page
        // state stays at zero and the render shows the unavailable line.
        assert_eq!(state.wifi.saved_count, 0);
    }

    #[test]
    fn wifi_pointer_selects_scan_and_saved_rows() {
        let mut state = wifi_state();
        state.wifi.scans[0] = secured_scan();
        state.wifi.scan_count = 1;

        // Row 0 hitbox selects the scan row.
        let changed =
            handle_wifi_pointer_down(rt::INVALID_HANDLE, &mut state, 40, WIFI_SCAN_ROW_Y0 + 4)
                .expect("row handled");
        assert!(changed);
        assert_eq!(state.wifi.selected_scan, 0);

        // Below the rows: no selection change, still handled.
        let _ = handle_wifi_pointer_down(rt::INVALID_HANDLE, &mut state, 40, WIFI_SCAN_ROW_Y0 + 40);
        assert_eq!(state.wifi.selected_scan, 0);
    }

    #[test]
    fn wifi_join_button_routes_secured_to_psk_prompt() {
        let mut state = wifi_state();
        state.wifi.scans[0] = secured_scan();
        state.wifi.scan_count = 1;

        let changed = handle_wifi_pointer_down(
            rt::INVALID_HANDLE,
            &mut state,
            WIFI_JOIN_BTN_X0 + 4,
            WIFI_BTN_Y0 + 4,
        )
        .expect("join handled");
        assert!(changed);
        assert_eq!(state.wifi.prompt, Some(WifiPrompt::JoinPsk));
        assert_eq!(state.wifi.prompt_len, 0);

        // Typing lands in the prompt buffer via the text path.
        for ch in b"password12" {
            let _ = append_text_input(&mut state, *ch as u64);
        }
        assert_eq!(state.wifi.prompt_len, 10);

        // Enter attempts the join (a bogus handle never reaches a service
        // and the transport reports InvalidArgument) and closes the prompt
        // with an honest outcome recorded.
        let _ = handle_key_down(
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            &mut state,
            28,
        );
        assert!(state.wifi.prompt.is_none());
        assert_eq!(state.wifi.join_outcome, Some(Err(WifiOpError::Invalid)));
    }

    #[test]
    fn wifi_join_button_open_network_skips_prompt() {
        let mut state = wifi_state();
        state.wifi.scans[0] = secured_scan();
        state.wifi.scans[0].security = rt::WifiSecurity::Open;
        state.wifi.scan_count = 1;

        let _ = handle_wifi_pointer_down(
            rt::INVALID_HANDLE,
            &mut state,
            WIFI_JOIN_BTN_X0 + 4,
            WIFI_BTN_Y0 + 4,
        );
        assert!(state.wifi.prompt.is_none());
        assert!(state.wifi.join_outcome.is_some());
    }

    #[test]
    fn wifi_saved_add_button_opens_prompt_and_esc_cancels() {
        let mut state = wifi_state();

        let _ = handle_wifi_pointer_down(
            rt::INVALID_HANDLE,
            &mut state,
            WIFI_ADD_BTN_X0 + 4,
            WIFI_ACTION_Y0 + 4,
        );
        assert_eq!(state.wifi.prompt, Some(WifiPrompt::SavedSsid));

        // Esc cancels the whole flow.
        let _ = handle_key_down(
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            &mut state,
            1,
        );
        assert!(state.wifi.prompt.is_none());
        assert_eq!(state.wifi.prompt_len, 0);
    }

    #[test]
    fn wifi_saved_add_walks_ssid_then_psk_stages() {
        let mut state = wifi_state();
        state.wifi.prompt = Some(WifiPrompt::SavedSsid);

        for ch in b"net" {
            let _ = append_text_input(&mut state, *ch as u64);
        }
        let _ = handle_key_down(
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            &mut state,
            28,
        );
        assert_eq!(state.wifi.prompt, Some(WifiPrompt::SavedPsk));
        assert_eq!(state.wifi.add_ssid_len, 3);

        for ch in b"password12" {
            let _ = append_text_input(&mut state, *ch as u64);
        }
        let _ = handle_key_down(
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            &mut state,
            28,
        );
        assert_eq!(state.wifi.prompt, Some(WifiPrompt::SavedPriority));

        // Empty priority means 0; enter commits the add (transport fails
        // honestly on the invalid handle) and reloads the saved list.
        let _ = handle_key_down(
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            &mut state,
            28,
        );
        assert!(state.wifi.prompt.is_none());
        assert!(state.wifi.saved_add_outcome.is_some());
    }

    #[test]
    fn wifi_prompt_is_modal_over_underlying_buttons() {
        let mut state = wifi_state();
        state.wifi.prompt = Some(WifiPrompt::JoinPsk);

        // Click on the SCAN button coordinates while the prompt is open:
        // outside the overlay, so the prompt closes and the button does
        // not fire (scan list stays untouched).
        let _ = handle_wifi_pointer_down(
            rt::INVALID_HANDLE,
            &mut state,
            WIFI_SCAN_BTN_X0 + 4,
            WIFI_BTN_Y0 + 4,
        );
        assert!(state.wifi.prompt.is_none());
        assert_eq!(state.wifi.scan_count, 0);
        assert_eq!(state.wifi.scan_error, None);
    }

    #[test]
    fn wifi_backspace_edits_prompt_via_key_path() {
        let mut state = wifi_state();
        state.wifi.prompt = Some(WifiPrompt::JoinPsk);
        let _ = append_text_input(&mut state, b'a' as u64);
        let _ = append_text_input(&mut state, b'b' as u64);
        assert_eq!(state.wifi.prompt_len, 2);
        let _ = handle_key_down(
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            &mut state,
            14,
        );
        assert_eq!(state.wifi.prompt_len, 1);
        assert_eq!(state.wifi.prompt_edit[..1], *b"a");
    }

    #[test]
    fn wifi_arrow_keys_move_scan_selection() {
        let mut state = wifi_state();
        state.wifi.scans[0] = secured_scan();
        state.wifi.scans[1] = secured_scan();
        state.wifi.scan_count = 2;

        let _ = handle_key_down(
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            &mut state,
            108,
        );
        assert_eq!(state.wifi.selected_scan, 1);
        let _ = handle_key_down(
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            &mut state,
            108,
        );
        assert_eq!(state.wifi.selected_scan, 1);
        let _ = handle_key_down(
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            &mut state,
            103,
        );
        assert_eq!(state.wifi.selected_scan, 0);
        let _ = handle_key_down(
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            &mut state,
            103,
        );
        assert_eq!(state.wifi.selected_scan, 0);
    }

    #[test]
    fn wifi_scan_and_join_on_invalid_handle_stay_honest() {
        let mut state = wifi_state();
        let _ = handle_wifi_pointer_down(
            rt::INVALID_HANDLE,
            &mut state,
            WIFI_SCAN_BTN_X0 + 4,
            WIFI_BTN_Y0 + 4,
        );
        // A bogus handle never reaches a service: the scan records the
        // failure and never fabricates rows.
        assert_eq!(state.wifi.scan_count, 0);
        assert_eq!(state.wifi.scan_total, 0);
        assert_eq!(state.wifi.scan_error, Some(WifiOpError::Invalid));

        state.wifi.scans[0] = secured_scan();
        state.wifi.scan_count = 1;
        state.wifi.prompt = Some(WifiPrompt::JoinPsk);
        for ch in b"password12" {
            let _ = wifi::wifi_prompt_char(&mut state.wifi, *ch as char);
        }
        let _ = handle_key_down(
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            rt::INVALID_HANDLE,
            &mut state,
            28,
        );
        assert_eq!(state.wifi.join_outcome, Some(Err(WifiOpError::Invalid)));
    }
}
