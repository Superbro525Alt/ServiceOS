use core::char;

use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;
use rt::PermissionPolicyState;

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
            Ok(()) if message.tag == rt::AppControlTag::FocusChanged as u32 && message.word_count > 0 => {
                state.focused = message.words[0] != 0;
                changed = true;
            }
            Ok(()) if message.tag == rt::AppControlTag::Resize as u32 && message.word_count >= 2 => {
                state.width = message.words[0] as u32;
                state.height = message.words[1] as u32;
                changed = true;
            }
            Ok(()) if message.tag == rt::AppControlTag::Pointer as u32 && message.word_count >= 4 => {
                let action = ui::decode_app_pointer_action(message.words[0]);
                let x = message.words[1] as i64 as i32;
                let y = message.words[2] as i64 as i32;
                if matches!(action, Some(rt::AppPointerAction::Down)) {
                    changed |= handle_pointer_down(
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
                if matches!(ui::decode_app_key_action(message.words[0]), Some(rt::AppKeyAction::Down)) {
                    changed |= handle_key_down(security_handle, state, message.words[1] as u32)?;
                }
            }
            Ok(()) if message.tag == rt::AppControlTag::Text as u32 && message.word_count > 0 => {
                if state.editing_note && let Some(ch) = char::from_u32(message.words[0] as u32) {
                    if ch == '\n' {
                        state.editing_note = false;
                        changed = true;
                    } else if ch.is_ascii_graphic() || ch == ' ' {
                        let mut scratch = [0u8; 4];
                        let bytes = ch.encode_utf8(&mut scratch).as_bytes();
                        if state.note_len + bytes.len() <= NOTE_MAX_BYTES {
                            state.note[state.note_len..state.note_len + bytes.len()].copy_from_slice(bytes);
                            state.note_len += bytes.len();
                            changed = true;
                        }
                    }
                }
            }
            Ok(()) if message.tag == rt::AppControlTag::Close as u32 => return Ok(ControlFlow::Exit),
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

fn handle_pointer_down(
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
        return Ok(true);
    }
    if x >= TAB_SECURITY_X0 && x < TAB_SECURITY_X1 && y >= TAB_Y0 && y < TAB_Y1 {
        state.page = SettingsPage::Security;
        state.editing_note = false;
        return Ok(true);
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
        update_policy(security_handle, state.selected_policy_index, PermissionPolicyState::Allowed)?;
        return Ok(true);
    }
    if x >= SEC_BLOCK_X0 && x < SEC_BLOCK_X1 && y >= SEC_ACTION_Y0 && y < SEC_ACTION_Y1 {
        update_policy(security_handle, state.selected_policy_index, PermissionPolicyState::Blocked)?;
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
            rt::runtime_env_decide(runtime_handle, runtime.env_id, PermissionPolicyState::Allowed)?;
        }
        return Ok(true);
    }
    if x >= SEC_DENY_X0 && x < SEC_DENY_X1 && y >= SEC_RUNTIME_Y0 && y < SEC_RUNTIME_Y1 {
        if let Some(runtime) = first_actionable_runtime(runtime_handle)? {
            rt::runtime_env_decide(runtime_handle, runtime.env_id, PermissionPolicyState::Blocked)?;
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

fn handle_key_down(
    security_handle: rt::Handle,
    state: &mut AppState,
    key: u32,
) -> rt::Result<bool> {
    match key {
        14 if state.editing_note && state.note_len > 0 => {
            state.note_len -= 1;
            Ok(true)
        }
        15 => {
            state.page = match state.page {
                SettingsPage::System => SettingsPage::Security,
                SettingsPage::Security => SettingsPage::System,
            };
            state.editing_note = false;
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
