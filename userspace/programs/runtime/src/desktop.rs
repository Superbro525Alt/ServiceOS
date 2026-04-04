use crate::{
    channel_call, channel_create, channel_receive_blocking, channel_send, desktop_app_id_from_word,
    desktop_drag_mode_from_word, desktop_status_error, desktop_status_from_word, handle_close,
    pack_bytes, rights, unpack_bytes, unpack_i32_pair, unpack_u32_pair, DesktopAppId, DesktopAppInfo,
    DesktopDragMode, DesktopInputAction, DesktopNotificationInfo, DesktopShellStatusInfo,
    DesktopStatus, DesktopTag, DesktopWindowAction, DesktopWindowInfo, DesktopWorkspaceAction,
    DesktopWorkspaceInfo, Error, Handle, IPC_MAX_WORDS, RawMessage, Result,
};

pub fn desktop_status(desktop_handle: Handle) -> Result<DesktopShellStatusInfo> {
    let mut request = RawMessage::empty(DesktopTag::StatusRequest as u32);
    let response = channel_call(desktop_handle, &mut request)?;
    if response.tag != DesktopTag::StatusReply as u32 || response.word_count < 4 {
        return Err(Error::InvalidArgument);
    }
    match desktop_status_from_word(response.words[0]) {
        DesktopStatus::Ok => Ok(DesktopShellStatusInfo {
            session_id: response.words[1] as u32,
            focused_app: desktop_app_id_from_word(response.words[2]).ok(),
            running_apps: response.words[3] as u32,
            focused_surface: response.words.get(4).copied().unwrap_or(0) as u32,
            drag_mode: response
                .words
                .get(5)
                .copied()
                .map(desktop_drag_mode_from_word)
                .unwrap_or(DesktopDragMode::None),
            pointer_x: response
                .words
                .get(6)
                .copied()
                .map(|value| unpack_i32_pair(value).0)
                .unwrap_or(0),
            pointer_y: response
                .words
                .get(6)
                .copied()
                .map(|value| unpack_i32_pair(value).1)
                .unwrap_or(0),
            active_workspace: response.words.get(7).copied().unwrap_or(1) as u32,
            workspace_count: response.words.get(8).copied().unwrap_or(1) as u32,
            notification_count: response.words.get(9).copied().unwrap_or(0) as u32,
        }),
        status => Err(desktop_status_error(status)),
    }
}

pub fn desktop_list_apps(desktop_handle: Handle, apps: &mut [DesktopAppInfo]) -> Result<usize> {
    let mut filled = 0usize;
    let mut start = 0usize;

    loop {
        let mut request = RawMessage::empty(DesktopTag::ListAppsRequest as u32);
        request.word_count = 1;
        request.words[0] = start as u64;
        let response = channel_call(desktop_handle, &mut request)?;
        if response.tag != DesktopTag::ListAppsReply as u32 || response.word_count < 3 {
            return Err(Error::InvalidArgument);
        }
        match desktop_status_from_word(response.words[0]) {
            DesktopStatus::Ok => {}
            status => return Err(desktop_status_error(status)),
        }

        let count = response.words[1] as usize;
        let next = response.words[2] as usize;
        if filled + count > apps.len() || response.word_count as usize != 3 + count * 4 {
            return Err(Error::BufferTooSmall);
        }
        for page_index in 0..count {
            let base = 3 + page_index * 4;
            apps[filled + page_index] = DesktopAppInfo {
                app_id: desktop_app_id_from_word(response.words[base])
                    .map_err(|_| Error::InvalidArgument)?,
                running: response.words[base + 1] != 0,
                focused: response.words[base + 2] != 0,
                surface_id: response.words[base + 3] as u32,
            };
        }
        filled += count;
        if count == 0 || next <= start {
            return Ok(filled);
        }
        start = next;
    }
}

pub fn desktop_launch_app(desktop_handle: Handle, app_id: DesktopAppId) -> Result<u32> {
    let mut request = RawMessage::empty(DesktopTag::LaunchAppRequest as u32);
    request.word_count = 1;
    request.words[0] = app_id as u32 as u64;
    let response = channel_call(desktop_handle, &mut request)?;
    if response.tag != DesktopTag::LaunchAppReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    match desktop_status_from_word(response.words[0]) {
        DesktopStatus::Ok => Ok(response.words[1] as u32),
        status => Err(desktop_status_error(status)),
    }
}

pub fn desktop_focus_app(desktop_handle: Handle, app_id: DesktopAppId) -> Result<u32> {
    let mut request = RawMessage::empty(DesktopTag::FocusAppRequest as u32);
    request.word_count = 1;
    request.words[0] = app_id as u32 as u64;
    let response = channel_call(desktop_handle, &mut request)?;
    if response.tag != DesktopTag::FocusAppReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    match desktop_status_from_word(response.words[0]) {
        DesktopStatus::Ok => Ok(response.words[1] as u32),
        status => Err(desktop_status_error(status)),
    }
}

pub fn desktop_list_windows(
    desktop_handle: Handle,
    windows: &mut [DesktopWindowInfo],
) -> Result<usize> {
    let mut filled = 0usize;
    let mut start = 0u32;

    loop {
        let mut request = RawMessage::empty(DesktopTag::ListWindowsRequest as u32);
        request.word_count = 1;
        request.words[0] = start as u64;
        let response = channel_call(desktop_handle, &mut request)?;
        if response.tag != DesktopTag::ListWindowsReply as u32 || response.word_count < 3 {
            return Err(Error::InvalidArgument);
        }
        match desktop_status_from_word(response.words[0]) {
            DesktopStatus::Ok => {}
            status => return Err(desktop_status_error(status)),
        }

        let count = response.words[1] as usize;
        let next_start = response.words[2] as u32;
        if response.word_count as usize != 3 + count * 5 {
            return Err(Error::InvalidArgument);
        }
        if filled + count > windows.len() {
            return Err(Error::BufferTooSmall);
        }

        for index in 0..count {
            let base = 3 + index * 5;
            let flags = response.words[base + 2];
            let (x, y) = unpack_i32_pair(response.words[base + 3]);
            let (width, height) = unpack_u32_pair(response.words[base + 4]);
            windows[filled + index] = DesktopWindowInfo {
                app_id: desktop_app_id_from_word(response.words[base])
                    .map_err(|_| Error::InvalidArgument)?,
                surface_id: response.words[base + 1] as u32,
                x,
                y,
                width,
                height,
                z_order: (flags >> 32) as u32,
                focused: (flags & 0x1) != 0,
                minimized: (flags & 0x2) != 0,
                visible: (flags & 0x4) != 0,
            };
        }
        filled += count;
        if next_start == u32::MAX {
            break;
        }
        start = next_start;
    }

    Ok(filled)
}

pub fn desktop_window_action(
    desktop_handle: Handle,
    action: DesktopWindowAction,
    app_id: Option<DesktopAppId>,
    arg0: u64,
    arg1: u64,
) -> Result<u32> {
    let mut request = RawMessage::empty(DesktopTag::WindowActionRequest as u32);
    request.word_count = 4;
    request.words[0] = action as u32 as u64;
    request.words[1] = app_id.map(|value| value as u32 as u64).unwrap_or(0);
    request.words[2] = arg0;
    request.words[3] = arg1;
    let response = channel_call(desktop_handle, &mut request)?;
    if response.tag != DesktopTag::WindowActionReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    match desktop_status_from_word(response.words[0]) {
        DesktopStatus::Ok => Ok(response.words[1] as u32),
        status => Err(desktop_status_error(status)),
    }
}

pub fn desktop_focus_next(desktop_handle: Handle) -> Result<u32> {
    desktop_window_action(desktop_handle, DesktopWindowAction::FocusNext, None, 0, 0)
}

pub fn desktop_close_app(desktop_handle: Handle, app_id: DesktopAppId) -> Result<()> {
    let _ = desktop_window_action(
        desktop_handle,
        DesktopWindowAction::Close,
        Some(app_id),
        0,
        0,
    )?;
    Ok(())
}

pub fn desktop_minimize_app(desktop_handle: Handle, app_id: DesktopAppId) -> Result<u32> {
    desktop_window_action(
        desktop_handle,
        DesktopWindowAction::Minimize,
        Some(app_id),
        0,
        0,
    )
}

pub fn desktop_restore_app(desktop_handle: Handle, app_id: DesktopAppId) -> Result<u32> {
    desktop_window_action(
        desktop_handle,
        DesktopWindowAction::Restore,
        Some(app_id),
        0,
        0,
    )
}

pub fn desktop_maximize_app(desktop_handle: Handle, app_id: DesktopAppId) -> Result<u32> {
    desktop_window_action(
        desktop_handle,
        DesktopWindowAction::Maximize,
        Some(app_id),
        0,
        0,
    )
}

pub fn desktop_move_app(
    desktop_handle: Handle,
    app_id: DesktopAppId,
    x: i32,
    y: i32,
) -> Result<u32> {
    desktop_window_action(
        desktop_handle,
        DesktopWindowAction::Move,
        Some(app_id),
        x as i64 as u64,
        y as i64 as u64,
    )
}

pub fn desktop_resize_app(
    desktop_handle: Handle,
    app_id: DesktopAppId,
    width: u32,
    height: u32,
) -> Result<u32> {
    desktop_window_action(
        desktop_handle,
        DesktopWindowAction::Resize,
        Some(app_id),
        width as u64,
        height as u64,
    )
}

pub fn desktop_notify(desktop_handle: Handle, text: &str) -> Result<()> {
    let text_bytes = text.as_bytes();
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(1)) * 8;
    if text_bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }
    let mut request = RawMessage::empty(DesktopTag::NotifyRequest as u32);
    request.word_count = 1 + pack_bytes(text_bytes, &mut request.words[1..])?;
    request.words[0] = text_bytes.len() as u64;
    let response = channel_call(desktop_handle, &mut request)?;
    if response.tag != DesktopTag::NotifyReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match desktop_status_from_word(response.words[0]) {
        DesktopStatus::Ok => Ok(()),
        status => Err(desktop_status_error(status)),
    }
}

pub fn desktop_notification_history(
    desktop_handle: Handle,
    index: u32,
) -> Result<DesktopNotificationInfo> {
    let mut request = RawMessage::empty(DesktopTag::NotificationHistoryRequest as u32);
    request.word_count = 1;
    request.words[0] = index as u64;
    let response = channel_call(desktop_handle, &mut request)?;
    if response.tag != DesktopTag::NotificationHistoryReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match desktop_status_from_word(response.words[0]) {
        DesktopStatus::Ok => {
            if response.word_count < 5 {
                return Err(Error::InvalidArgument);
            }
            let len = response.words[4] as usize;
            let mut text = [0u8; 64];
            unpack_bytes(&response.words[5..response.word_count as usize], len, &mut text)?;
            Ok(DesktopNotificationInfo {
                sequence: response.words[1] as u32,
                source_app: desktop_app_id_from_word(response.words[2]).ok(),
                actionable: response.words[3] != 0,
                text_len: len as u32,
                text,
            })
        }
        status => Err(desktop_status_error(status)),
    }
}

pub fn desktop_workspace_action(
    desktop_handle: Handle,
    action: DesktopWorkspaceAction,
    workspace_id: u32,
) -> Result<DesktopWorkspaceInfo> {
    let mut request = RawMessage::empty(DesktopTag::WorkspaceRequest as u32);
    request.word_count = 2;
    request.words[0] = action as u32 as u64;
    request.words[1] = workspace_id as u64;
    let response = channel_call(desktop_handle, &mut request)?;
    if response.tag != DesktopTag::WorkspaceReply as u32 || response.word_count < 4 {
        return Err(Error::InvalidArgument);
    }
    match desktop_status_from_word(response.words[0]) {
        DesktopStatus::Ok => Ok(DesktopWorkspaceInfo {
            active_workspace: response.words[1] as u32,
            workspace_count: response.words[2] as u32,
            focused_surface: response.words[3] as u32,
        }),
        status => Err(desktop_status_error(status)),
    }
}

pub fn desktop_open_path(desktop_handle: Handle, path: &str) -> Result<u32> {
    if path.len() > IPC_MAX_WORDS * 8 {
        return Err(Error::BufferTooSmall);
    }
    let mut request = RawMessage::empty(DesktopTag::OpenPathRequest as u32);
    request.word_count = 1 + pack_bytes(path.as_bytes(), &mut request.words[1..])?;
    request.words[0] = path.len() as u64;
    let response = channel_call(desktop_handle, &mut request)?;
    if response.tag != DesktopTag::OpenPathReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    match desktop_status_from_word(response.words[0]) {
        DesktopStatus::Ok => Ok(response.words[1] as u32),
        status => Err(desktop_status_error(status)),
    }
}

fn desktop_input_request(
    desktop_handle: Handle,
    action: DesktopInputAction,
    x: i32,
    y: i32,
    detail: i32,
    expect_reply: bool,
) -> Result<Option<u32>> {
    let mut request = RawMessage::empty(DesktopTag::InputRequest as u32);
    request.word_count = 4;
    request.words[0] = action as u32 as u64;
    request.words[1] = x as i64 as u64;
    request.words[2] = y as i64 as u64;
    request.words[3] = detail as i64 as u64;
    let mut reply = None;
    if expect_reply {
        let pair = channel_create()?;
        request.handle_count = 1;
        request.handles[0] = pair.second;
        request.handle_rights[0] = rights::SEND;
        reply = Some(pair);
    }
    channel_send(desktop_handle, &request)?;
    if !expect_reply {
        return Ok(None);
    }

    let reply = reply.expect("reply pair for reply-expected desktop input");
    let _ = handle_close(reply.second);
    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != DesktopTag::InputReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    match desktop_status_from_word(response.words[0]) {
        DesktopStatus::Ok => Ok(Some(response.words[1] as u32)),
        status => Err(desktop_status_error(status)),
    }
}

pub fn desktop_pointer_input(
    desktop_handle: Handle,
    action: DesktopInputAction,
    x: i32,
    y: i32,
) -> Result<u32> {
    desktop_input_request(desktop_handle, action, x, y, 0, true)
        .map(|surface| surface.unwrap_or(0))
}

pub fn desktop_pointer_input_async(
    desktop_handle: Handle,
    action: DesktopInputAction,
    x: i32,
    y: i32,
) -> Result<()> {
    desktop_input_request(desktop_handle, action, x, y, 0, false).map(|_| ())
}

pub fn desktop_pointer_scroll_input_async(
    desktop_handle: Handle,
    x: i32,
    y: i32,
    delta_y: i32,
) -> Result<()> {
    desktop_input_request(
        desktop_handle,
        DesktopInputAction::PointerScroll,
        x,
        y,
        delta_y,
        false,
    )
    .map(|_| ())
}

pub fn desktop_pointer_click(desktop_handle: Handle, x: i32, y: i32) -> Result<u32> {
    desktop_pointer_input(desktop_handle, DesktopInputAction::Click, x, y)
}

pub fn desktop_key_input(
    desktop_handle: Handle,
    action: DesktopInputAction,
    key_code: u32,
    value: u32,
) -> Result<u32> {
    desktop_pointer_input(desktop_handle, action, key_code as i32, value as i32)
}

pub fn desktop_key_input_async(
    desktop_handle: Handle,
    action: DesktopInputAction,
    key_code: u32,
    value: u32,
) -> Result<()> {
    desktop_pointer_input_async(desktop_handle, action, key_code as i32, value as i32)
}
