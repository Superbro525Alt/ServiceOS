use rt::{
    ControlTag, DesktopAppId, DesktopDragMode, DesktopInputAction, DesktopStatus, DesktopTag,
    DesktopWindowAction, LifecycleEvent, RawMessage,
};
use serviceos_userspace_runtime as rt;

use crate::{
    DesktopState, SESSION_ID,
    input::{focus_next_app, handle_input},
    windows::{
        close_app, deliver_open_intent, encode_window_page, focus_app, focused_surface_id,
        launch_or_focus_app, maximize_app, minimize_app, move_app, move_focused_to_workspace,
        open_path_in_files, parse_content_intent, post_notification, refresh_apps, resize_app,
        restore_app, running_app_count, switch_workspace,
    },
};

pub(crate) fn coalescible_pointer_move(request: &RawMessage) -> Option<(i32, i32, i32)> {
    if request.tag != DesktopTag::InputRequest as u32
        || request.word_count < 4
        || request.handle_count != 0
    {
        return None;
    }
    match desktop_input_action_from_word(request.words[0]) {
        Some(DesktopInputAction::PointerMove) => Some((
            request.words[1] as i64 as i32,
            request.words[2] as i64 as i32,
            request.words[3] as i64 as i32,
        )),
        _ => None,
    }
}

pub(crate) fn dispatch_input_request(
    state: &mut DesktopState,
    action: DesktopInputAction,
    x: i32,
    y: i32,
    detail: i32,
    reply_handle: Option<rt::Handle>,
) -> rt::Result<()> {
    let result = handle_input(state, action, x, y, detail);
    if let Some(reply_handle) = reply_handle {
        let mut reply = RawMessage::empty(DesktopTag::InputReply as u32);
        reply.word_count = 2;
        reply_for_surface(&mut reply, result);
        let _ = rt::channel_send(reply_handle, &reply);
        let _ = rt::handle_close(reply_handle);
        Ok(())
    } else {
        result.map(|_| ())
    }
}

pub(crate) fn handle_request(state: &mut DesktopState, request: &RawMessage) -> rt::Result<()> {
    match request.tag {
        x if x == DesktopTag::StatusRequest as u32 => {
            if request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(DesktopTag::StatusReply as u32);
            reply.word_count = 10;
            reply.words[0] = DesktopStatus::Ok as u32 as u64;
            reply.words[1] = SESSION_ID as u64;
            reply.words[2] = state.focused_app.map(|app| app as u32 as u64).unwrap_or(0);
            reply.words[3] = running_app_count(&state.apps) as u64;
            reply.words[4] = focused_surface_id(state) as u64;
            reply.words[5] = state
                .drag_state
                .map(|drag| drag.mode())
                .unwrap_or(DesktopDragMode::None) as u32 as u64;
            reply.words[6] = crate::windows::pack_i32_pair(state.pointer_x, state.pointer_y);
            reply.words[7] = state.active_workspace as u64;
            reply.words[8] = crate::WORKSPACE_COUNT as u64;
            reply.words[9] = state.notification_history_len as u64;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == DesktopTag::ListAppsRequest as u32 => {
            if request.handle_count < 1 {
                return Ok(());
            }
            let start = request.words[0] as usize;
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(DesktopTag::ListAppsReply as u32);
            reply.word_count = 3;
            reply.words[0] = DesktopStatus::Ok as u32 as u64;
            let (end, count) = crate::list_apps_page(start, state.apps.len());
            reply.words[1] = count as u64;
            reply.words[2] = end as u64;
            for (page_index, slot) in state.apps[start..end].iter().copied().enumerate() {
                let base = 3 + page_index * 4;
                reply.words[base] = slot.app_id as u32 as u64;
                reply.words[base + 1] = u64::from(slot.running);
                reply.words[base + 2] = u64::from(state.focused_app == Some(slot.app_id));
                reply.words[base + 3] = slot.window.surface_id as u64;
            }
            reply.word_count += (count as u32) * 4;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == DesktopTag::LaunchAppRequest as u32 => {
            if request.word_count < 1 || request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(DesktopTag::LaunchAppReply as u32);
            reply.word_count = 2;
            match desktop_app_from_word(request.words[0]) {
                Some(app_id) => reply_for_surface(&mut reply, launch_or_focus_app(state, app_id)),
                None => reply.words[0] = DesktopStatus::NotFound as u32 as u64,
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == DesktopTag::FocusAppRequest as u32 => {
            if request.word_count < 1 || request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(DesktopTag::FocusAppReply as u32);
            reply.word_count = 2;
            match desktop_app_from_word(request.words[0]) {
                Some(app_id) => reply_for_surface(&mut reply, focus_app(state, app_id)),
                None => reply.words[0] = DesktopStatus::NotFound as u32 as u64,
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == DesktopTag::ListWindowsRequest as u32 => {
            if request.handle_count < 1 {
                return Ok(());
            }
            let start = request.words[0] as usize;
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(DesktopTag::ListWindowsReply as u32);
            reply.word_count = 3;
            reply.words[0] = DesktopStatus::Ok as u32 as u64;
            encode_window_page(state, start, &mut reply);
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == DesktopTag::WindowActionRequest as u32 => {
            if request.word_count < 4 || request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(DesktopTag::WindowActionReply as u32);
            reply.word_count = 2;
            let action = desktop_window_action_from_word(request.words[0]);
            let app_id = desktop_app_from_word(request.words[1]);
            let result = match action {
                Some(DesktopWindowAction::Focus) => app_id
                    .ok_or(rt::Error::NotFound)
                    .and_then(|app| focus_app(state, app)),
                Some(DesktopWindowAction::Close) => app_id
                    .ok_or(rt::Error::NotFound)
                    .and_then(|app| close_app(state, app).map(|_| 0)),
                Some(DesktopWindowAction::Minimize) => app_id
                    .ok_or(rt::Error::NotFound)
                    .and_then(|app| minimize_app(state, app)),
                Some(DesktopWindowAction::Restore) => app_id
                    .ok_or(rt::Error::NotFound)
                    .and_then(|app| restore_app(state, app)),
                Some(DesktopWindowAction::Move) => {
                    app_id.ok_or(rt::Error::NotFound).and_then(|app| {
                        move_app(
                            state,
                            app,
                            request.words[2] as i64 as i32,
                            request.words[3] as i64 as i32,
                        )
                    })
                }
                Some(DesktopWindowAction::Resize) => {
                    app_id.ok_or(rt::Error::NotFound).and_then(|app| {
                        resize_app(state, app, request.words[2] as u32, request.words[3] as u32)
                    })
                }
                Some(DesktopWindowAction::Maximize) => app_id
                    .ok_or(rt::Error::NotFound)
                    .and_then(|app| maximize_app(state, app)),
                Some(DesktopWindowAction::FocusNext) => focus_next_app(state),
                None => Err(rt::Error::NotFound),
            };
            reply_for_surface(&mut reply, result);
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == DesktopTag::InputRequest as u32 => {
            if request.word_count < 4 {
                return Ok(());
            }
            let action = desktop_input_action_from_word(request.words[0]);
            let x = request.words[1] as i64 as i32;
            let y = request.words[2] as i64 as i32;
            let detail = request.words[3] as i64 as i32;
            match action {
                Some(action) => dispatch_input_request(
                    state,
                    action,
                    x,
                    y,
                    detail,
                    if request.handle_count >= 1 {
                        Some(request.handles[0])
                    } else {
                        None
                    },
                )?,
                None => {
                    if request.handle_count >= 1 {
                        let reply_handle = request.handles[0];
                        let mut reply = RawMessage::empty(DesktopTag::InputReply as u32);
                        reply.word_count = 2;
                        reply.words[0] = DesktopStatus::NotFound as u32 as u64;
                        reply.words[1] = 0;
                        let _ = rt::channel_send(reply_handle, &reply);
                        let _ = rt::handle_close(reply_handle);
                    }
                }
            }
        }
        x if x == DesktopTag::NotifyRequest as u32 => {
            if request.handle_count < 1 || request.word_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let text_len = request.words[0] as usize;
            let mut text = [0u8; crate::MAX_NOTIFICATION_BYTES];
            let status = if text_len > crate::MAX_NOTIFICATION_BYTES {
                DesktopStatus::Busy
            } else if unpack_bytes(
                &request.words[1..request.word_count as usize],
                text_len,
                &mut state.notification,
            )
            .is_err()
            {
                DesktopStatus::Busy
            } else {
                text[..text_len].copy_from_slice(&state.notification[..text_len]);
                match handle_content_intent_or_notify(state, &text[..text_len]) {
                    ContentOutcome::Notified | ContentOutcome::Accepted => DesktopStatus::Ok,
                    ContentOutcome::Rejected => DesktopStatus::Busy,
                }
            };
            let mut reply = RawMessage::empty(DesktopTag::NotifyReply as u32);
            reply.word_count = 1;
            reply.words[0] = status as u32 as u64;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == DesktopTag::NotificationHistoryRequest as u32 => {
            if request.handle_count < 1 || request.word_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let index = request.words[0] as usize;
            let mut reply = RawMessage::empty(DesktopTag::NotificationHistoryReply as u32);
            if let Some(entry) = state
                .notification_history
                .iter()
                .copied()
                .take(state.notification_history_len)
                .nth(index)
            {
                reply.word_count =
                    5 + pack_bytes(&entry.text[..entry.text_len], &mut reply.words[5..])?;
                reply.words[0] = DesktopStatus::Ok as u32 as u64;
                reply.words[1] = entry.sequence as u64;
                reply.words[2] = entry
                    .source_app
                    .map(|value| value as u32 as u64)
                    .unwrap_or(0);
                reply.words[3] = u64::from(entry.actionable);
                reply.words[4] = entry.text_len as u64;
            } else {
                reply.word_count = 1;
                reply.words[0] = DesktopStatus::NotFound as u32 as u64;
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == DesktopTag::WorkspaceRequest as u32 => {
            if request.handle_count < 1 || request.word_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let action = request.words[0] as u32;
            let workspace = request.words.get(1).copied().unwrap_or(1) as u32;
            let result = match action {
                x if x == rt::DesktopWorkspaceAction::Status as u32 => {
                    Ok(focused_surface_id(state))
                }
                x if x == rt::DesktopWorkspaceAction::Switch as u32 => {
                    switch_workspace(state, workspace)
                }
                x if x == rt::DesktopWorkspaceAction::MoveFocused as u32 => {
                    move_focused_to_workspace(state, workspace)
                }
                _ => Err(rt::Error::NotFound),
            };
            let mut reply = RawMessage::empty(DesktopTag::WorkspaceReply as u32);
            reply.word_count = 4;
            match result {
                Ok(surface_id) => {
                    reply.words[0] = DesktopStatus::Ok as u32 as u64;
                    reply.words[1] = state.active_workspace as u64;
                    reply.words[2] = crate::WORKSPACE_COUNT as u64;
                    reply.words[3] = surface_id as u64;
                }
                Err(rt::Error::PermissionDenied) => {
                    reply.words[0] = DesktopStatus::Denied as u32 as u64
                }
                Err(rt::Error::NotFound) => reply.words[0] = DesktopStatus::NotFound as u32 as u64,
                Err(_) => reply.words[0] = DesktopStatus::Busy as u32 as u64,
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == DesktopTag::OpenPathRequest as u32 => {
            if request.handle_count < 1 || request.word_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let path_len = request.words[0] as usize;
            let mut path = [0u8; rt::IPC_MAX_WORDS * 8];
            let mut reply = RawMessage::empty(DesktopTag::OpenPathReply as u32);
            reply.word_count = 2;
            let result = if unpack_bytes(
                &request.words[1..request.word_count as usize],
                path_len,
                &mut path,
            )
            .is_ok()
            {
                core::str::from_utf8(&path[..path_len])
                    .map_err(|_| rt::Error::InvalidArgument)
                    .and_then(|value| open_path_in_files(state, value))
            } else {
                Err(rt::Error::InvalidArgument)
            };
            reply_for_surface(&mut reply, result);
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        _ => {}
    }

    Ok(())
}

fn unpack_bytes(words: &[u64], len: usize, destination: &mut [u8]) -> rt::Result<()> {
    if len > destination.len() || len > words.len() * 8 {
        return Err(rt::Error::BufferTooSmall);
    }
    let mut copied = 0usize;
    for word in words.iter().copied() {
        if copied >= len {
            break;
        }
        let bytes = word.to_le_bytes();
        let chunk = (len - copied).min(bytes.len());
        destination[copied..copied + chunk].copy_from_slice(&bytes[..chunk]);
        copied += chunk;
    }
    Ok(())
}

fn pack_bytes(source: &[u8], words: &mut [u64]) -> rt::Result<u32> {
    let required = source.len().div_ceil(8);
    if required > words.len() {
        return Err(rt::Error::BufferTooSmall);
    }
    for (index, chunk) in source.chunks(8).enumerate() {
        let mut bytes = [0u8; 8];
        bytes[..chunk.len()].copy_from_slice(chunk);
        words[index] = u64::from_le_bytes(bytes);
    }
    Ok(required as u32)
}

fn reply_for_surface(reply: &mut RawMessage, result: rt::Result<u32>) {
    match result {
        Ok(surface_id) => {
            reply.words[0] = DesktopStatus::Ok as u32 as u64;
            reply.words[1] = surface_id as u64;
        }
        Err(rt::Error::PermissionDenied) => reply.words[0] = DesktopStatus::Denied as u32 as u64,
        Err(rt::Error::NotFound) => reply.words[0] = DesktopStatus::NotFound as u32 as u64,
        Err(_) => reply.words[0] = DesktopStatus::Busy as u32 as u64,
    }
}

fn desktop_window_action_from_word(value: u64) -> Option<DesktopWindowAction> {
    match value as u32 {
        x if x == DesktopWindowAction::Focus as u32 => Some(DesktopWindowAction::Focus),
        x if x == DesktopWindowAction::Close as u32 => Some(DesktopWindowAction::Close),
        x if x == DesktopWindowAction::Minimize as u32 => Some(DesktopWindowAction::Minimize),
        x if x == DesktopWindowAction::Restore as u32 => Some(DesktopWindowAction::Restore),
        x if x == DesktopWindowAction::Move as u32 => Some(DesktopWindowAction::Move),
        x if x == DesktopWindowAction::Resize as u32 => Some(DesktopWindowAction::Resize),
        x if x == DesktopWindowAction::FocusNext as u32 => Some(DesktopWindowAction::FocusNext),
        x if x == DesktopWindowAction::Maximize as u32 => Some(DesktopWindowAction::Maximize),
        _ => None,
    }
}

fn desktop_app_from_word(value: u64) -> Option<DesktopAppId> {
    match value as u32 {
        x if x == DesktopAppId::Settings as u32 => Some(DesktopAppId::Settings),
        x if x == DesktopAppId::Files as u32 => Some(DesktopAppId::Files),
        x if x == DesktopAppId::Monitor as u32 => Some(DesktopAppId::Monitor),
        x if x == DesktopAppId::Terminal as u32 => Some(DesktopAppId::Terminal),
        x if x == DesktopAppId::SoftwareCenter as u32 => Some(DesktopAppId::SoftwareCenter),
        x if x == DesktopAppId::Media as u32 => Some(DesktopAppId::Media),
        _ => None,
    }
}

fn desktop_input_action_from_word(value: u64) -> Option<DesktopInputAction> {
    match value as u32 {
        x if x == DesktopInputAction::PointerDown as u32 => Some(DesktopInputAction::PointerDown),
        x if x == DesktopInputAction::PointerMove as u32 => Some(DesktopInputAction::PointerMove),
        x if x == DesktopInputAction::PointerUp as u32 => Some(DesktopInputAction::PointerUp),
        x if x == DesktopInputAction::Click as u32 => Some(DesktopInputAction::Click),
        x if x == DesktopInputAction::KeyDown as u32 => Some(DesktopInputAction::KeyDown),
        x if x == DesktopInputAction::KeyUp as u32 => Some(DesktopInputAction::KeyUp),
        x if x == DesktopInputAction::TextInput as u32 => Some(DesktopInputAction::TextInput),
        x if x == DesktopInputAction::PointerScroll as u32 => {
            Some(DesktopInputAction::PointerScroll)
        }
        _ => None,
    }
}

/// Outcome of inspecting notify-channel text for a reserved content intent.
enum ContentOutcome {
    /// Plain notification text; was posted as usual.
    Notified,
    /// Content intent accepted (drag armed or open-with delivered).
    Accepted,
    /// Content-intent framing but malformed or undeliverable payload.
    Rejected,
}

fn handle_content_intent_or_notify(state: &mut DesktopState, text: &[u8]) -> ContentOutcome {
    let Some(intent) = parse_content_intent(text) else {
        if post_notification(state, None, false, text).is_err() {
            return ContentOutcome::Rejected;
        }
        return ContentOutcome::Notified;
    };
    match intent.target_app() {
        Some(app_id) => {
            let path = core::str::from_utf8(intent.path_bytes())
                .map(|value| value.trim_matches(char::is_whitespace))
                .unwrap_or("");
            if path.is_empty() || deliver_open_intent(state, app_id, path).is_err() {
                return ContentOutcome::Rejected;
            }
        }
        None => {
            let now = rt::monotonic_now().unwrap_or(0);
            state.content_drag = Some(crate::windows::ContentDrag {
                path_len: intent.path_len,
                path: intent.path,
                deadline: now.saturating_add(crate::windows::CONTENT_DRAG_TIMEOUT_TICKS),
            });
            state.pending_shell_refresh.set();
        }
    }
    ContentOutcome::Accepted
}

pub(crate) fn poll_lifecycle(bootstrap: rt::Handle) -> rt::Result<bool> {
    let mut message = RawMessage::empty(0);
    match rt::channel_receive_nonblocking(bootstrap, &mut message) {
        Ok(()) if message.tag == ControlTag::Lifecycle as u32 && message.word_count > 0 => {
            Ok(matches!(
                lifecycle_event_from_word(message.words[0]),
                LifecycleEvent::Restarting | LifecycleEvent::Stopped
            ))
        }
        Ok(()) => Ok(false),
        Err(rt::Error::QueueEmpty) => Ok(false),
        Err(error) => Err(error),
    }
}

fn lifecycle_event_from_word(value: u64) -> LifecycleEvent {
    match value as u32 {
        x if x == LifecycleEvent::Starting as u32 => LifecycleEvent::Starting,
        x if x == LifecycleEvent::Ready as u32 => LifecycleEvent::Ready,
        x if x == LifecycleEvent::Failed as u32 => LifecycleEvent::Failed,
        x if x == LifecycleEvent::Stopped as u32 => LifecycleEvent::Stopped,
        _ => LifecycleEvent::Restarting,
    }
}
