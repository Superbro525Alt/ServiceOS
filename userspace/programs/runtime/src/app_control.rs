use crate::{
    channel_send, AppControlTag, AppKeyAction, AppPointerAction, Handle, RawMessage, Result,
};

pub fn app_control_focus(control_handle: Handle, focused: bool) -> Result<()> {
    let mut request = RawMessage::empty(AppControlTag::FocusChanged as u32);
    request.word_count = 1;
    request.words[0] = u64::from(focused);
    channel_send(control_handle, &request)
}

pub fn app_control_resize(control_handle: Handle, width: u32, height: u32) -> Result<()> {
    let mut request = RawMessage::empty(AppControlTag::Resize as u32);
    request.word_count = 2;
    request.words[0] = width as u64;
    request.words[1] = height as u64;
    channel_send(control_handle, &request)
}

pub fn app_control_close(control_handle: Handle) -> Result<()> {
    let request = RawMessage::empty(AppControlTag::Close as u32);
    channel_send(control_handle, &request)
}

pub fn app_control_pointer(
    control_handle: Handle,
    action: AppPointerAction,
    x: i32,
    y: i32,
    button: u32,
    detail: i32,
) -> Result<()> {
    let mut request = RawMessage::empty(AppControlTag::Pointer as u32);
    request.word_count = 5;
    request.words[0] = action as u32 as u64;
    request.words[1] = x as i64 as u64;
    request.words[2] = y as i64 as u64;
    request.words[3] = button as u64;
    request.words[4] = detail as i64 as u64;
    channel_send(control_handle, &request)
}

pub fn app_control_key(
    control_handle: Handle,
    action: AppKeyAction,
    key_code: u32,
    modifiers: u32,
) -> Result<()> {
    let mut request = RawMessage::empty(AppControlTag::Key as u32);
    request.word_count = 3;
    request.words[0] = action as u32 as u64;
    request.words[1] = key_code as u64;
    request.words[2] = modifiers as u64;
    channel_send(control_handle, &request)
}

pub fn app_control_text(control_handle: Handle, scalar: char) -> Result<()> {
    let mut request = RawMessage::empty(AppControlTag::Text as u32);
    request.word_count = 1;
    request.words[0] = scalar as u32 as u64;
    channel_send(control_handle, &request)
}
