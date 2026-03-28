use serviceos_userspace_runtime as rt;
use rt::{
    ControlTag, DesktopAppId, DesktopDragMode, DesktopInputAction, DesktopStatus, DesktopTag,
    DesktopWindowAction, LifecycleEvent, RawMessage,
};

use crate::{
    input::{focus_next_app, handle_input},
    windows::{
        close_app, encode_window_page, focus_app, focused_surface_id, launch_or_focus_app,
        maximize_app, minimize_app, move_app, resize_app, restore_app, running_app_count,
    },
    DesktopState, SESSION_ID,
};

pub(crate) fn handle_request(state: &mut DesktopState, request: &RawMessage) -> rt::Result<()> {
    match request.tag {
        x if x == DesktopTag::StatusRequest as u32 => {
            if request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(DesktopTag::StatusReply as u32);
            reply.word_count = 7;
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
            let end = (start + crate::APP_PAGE_SIZE).min(state.apps.len());
            let count = end.saturating_sub(start);
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
                Some(DesktopWindowAction::Focus) => {
                    app_id.ok_or(rt::Error::NotFound).and_then(|app| focus_app(state, app))
                }
                Some(DesktopWindowAction::Close) => app_id
                    .ok_or(rt::Error::NotFound)
                    .and_then(|app| close_app(state, app).map(|_| 0)),
                Some(DesktopWindowAction::Minimize) => {
                    app_id.ok_or(rt::Error::NotFound).and_then(|app| minimize_app(state, app))
                }
                Some(DesktopWindowAction::Restore) => {
                    app_id.ok_or(rt::Error::NotFound).and_then(|app| restore_app(state, app))
                }
                Some(DesktopWindowAction::Move) => app_id.ok_or(rt::Error::NotFound).and_then(|app| {
                    move_app(
                        state,
                        app,
                        request.words[2] as i64 as i32,
                        request.words[3] as i64 as i32,
                    )
                }),
                Some(DesktopWindowAction::Resize) => {
                    app_id.ok_or(rt::Error::NotFound).and_then(|app| {
                        resize_app(state, app, request.words[2] as u32, request.words[3] as u32)
                    })
                }
                Some(DesktopWindowAction::Maximize) => {
                    app_id.ok_or(rt::Error::NotFound).and_then(|app| maximize_app(state, app))
                }
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
            let result = match action {
                Some(action) => handle_input(state, action, x, y, detail),
                None => Err(rt::Error::NotFound),
            };
            if request.handle_count >= 1 {
                let reply_handle = request.handles[0];
                let mut reply = RawMessage::empty(DesktopTag::InputReply as u32);
                reply.word_count = 2;
                reply_for_surface(&mut reply, result);
                let _ = rt::channel_send(reply_handle, &reply);
                let _ = rt::handle_close(reply_handle);
            }
        }
        _ => {}
    }

    Ok(())
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
        x if x == DesktopInputAction::PointerScroll as u32 => Some(DesktopInputAction::PointerScroll),
        _ => None,
    }
}

pub(crate) fn poll_lifecycle(bootstrap: rt::Handle) -> rt::Result<bool> {
    let mut message = RawMessage::empty(0);
    match rt::channel_receive_nonblocking(bootstrap, &mut message) {
        Ok(()) if message.tag == ControlTag::Lifecycle as u32 && message.word_count > 0 => Ok(
            matches!(
                lifecycle_event_from_word(message.words[0]),
                LifecycleEvent::Restarting | LifecycleEvent::Stopped
            ),
        ),
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
