use super::*;

pub(crate) enum ControlFlow {
    Continue,
    Exit,
}

pub(crate) enum TerminalMessage {
    Output(usize),
    Closed,
}

pub(crate) fn poll_control(
    control_handle: rt::Handle,
    state: &mut TerminalState,
    width: &mut u32,
    height: &mut u32,
    focused: &mut bool,
) -> rt::Result<(ControlFlow, bool, bool)> {
    let mut changed = false;
    let mut did_work = false;
    loop {
        let mut message = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(control_handle, &mut message) {
            Ok(()) if message.tag == AppControlTag::FocusChanged as u32 && message.word_count > 0 => {
                *focused = message.words[0] != 0;
                changed = true;
                did_work = true;
            }
            Ok(()) if message.tag == AppControlTag::Resize as u32 && message.word_count >= 2 => {
                *width = message.words[0] as u32;
                *height = message.words[1] as u32;
                changed = true;
                did_work = true;
            }
            Ok(()) if message.tag == AppControlTag::Close as u32 => {
                return Ok((ControlFlow::Exit, false, true));
            }
            Ok(()) if message.tag == AppControlTag::Pointer as u32 && message.word_count >= 5 => {
                did_work = true;
                let action = app_pointer_action_from_word(message.words[0]);
                let x = message.words[1] as i64 as i32;
                let y = message.words[2] as i64 as i32;
                let detail = message.words[4] as i64 as i32;
                match action {
                    Some(rt::AppPointerAction::Down) => changed |= handle_pointer_down(state, x, y),
                    Some(rt::AppPointerAction::Move) => changed |= handle_pointer_move(state, x, y),
                    Some(rt::AppPointerAction::Up) => changed |= handle_pointer_up(state, x, y),
                    Some(rt::AppPointerAction::Scroll) => {
                        handle_pointer_scroll(state, detail);
                        changed = true;
                    }
                    _ => {}
                }
            }
            Ok(()) if message.tag == AppControlTag::Text as u32 && message.word_count > 0 => {
                if let Some(ch) = core::char::from_u32(message.words[0] as u32) {
                    state.selection = None;
                    if let Some(tab) = crate::tabs::active_tab_mut(state) {
                        let mut bytes = [0u8; 4];
                        let encoded = ch.encode_utf8(&mut bytes);
                        tab.scroll_offset = 0;
                        let _ = rt::terminal_session_send_input(tab.session_handle, encoded.as_bytes());
                        changed = true;
                        did_work = true;
                    }
                }
            }
            Ok(()) if message.tag == AppControlTag::Key as u32 && message.word_count >= 2 => {
                did_work = true;
                if message.words[0] as u32 == rt::AppKeyAction::Down as u32 {
                    let key_code = message.words[1] as u32;
                    let modifiers = message.words.get(2).copied().unwrap_or(0) as u32;
                    changed |= handle_key_down(state, key_code, modifiers)?;
                }
            }
            Ok(()) => {}
            Err(rt::Error::QueueEmpty) => break,
            Err(error) => return Err(error),
        }
    }
    Ok((ControlFlow::Continue, changed, did_work))
}

pub(crate) fn handle_key_down(
    state: &mut TerminalState,
    key_code: u32,
    modifiers: u32,
) -> rt::Result<bool> {
    if modifiers & MOD_CTRL != 0 && modifiers & MOD_SHIFT != 0 {
        match key_code {
            KEY_T => {
                crate::tabs::open_new_tab(state)?;
                return Ok(true);
            }
            KEY_W => {
                crate::tabs::close_active_tab(state);
                return Ok(true);
            }
            KEY_C => {
                crate::render::copy_selection(state);
                return Ok(true);
            }
            KEY_V => {
                let _ = crate::render::paste_clipboard(state);
                return Ok(true);
            }
            KEY_1 => {
                state.theme_index = 0;
                return Ok(true);
            }
            KEY_2 => {
                state.theme_index = 1;
                return Ok(true);
            }
            KEY_3 => {
                state.theme_index = 2;
                return Ok(true);
            }
            _ => {}
        }
    }

    if modifiers & MOD_CTRL != 0 && key_code == KEY_TAB {
        if modifiers & MOD_SHIFT != 0 {
            crate::tabs::focus_previous_tab(state);
        } else {
            crate::tabs::focus_next_tab(state);
        }
        return Ok(true);
    }
    if modifiers & MOD_CTRL != 0 && key_code == KEY_C {
        state.selection = None;
        if let Some(tab) = crate::tabs::active_tab_mut(state) {
            tab.scroll_offset = 0;
            rt::terminal_session_send_input(tab.session_handle, &[0x03])?;
            return Ok(true);
        }
    }
    if key_code == KEY_PAGE_UP || (modifiers & MOD_SHIFT != 0 && key_code == KEY_UP) {
        let rows = state.rows;
        if let Some(tab) = crate::tabs::active_tab_mut(state) {
            crate::render::scroll_up_view(
                tab,
                if key_code == KEY_PAGE_UP { rows.saturating_sub(1).max(1) } else { 1 },
                rows,
            );
            return Ok(true);
        }
    }
    if key_code == KEY_PAGE_DOWN || (modifiers & MOD_SHIFT != 0 && key_code == KEY_DOWN) {
        let rows = state.rows;
        if let Some(tab) = crate::tabs::active_tab_mut(state) {
            crate::render::scroll_down_view(
                tab,
                if key_code == KEY_PAGE_DOWN { rows.saturating_sub(1).max(1) } else { 1 },
            );
            return Ok(true);
        }
    }

    state.selection = None;
    let Some(tab) = crate::tabs::active_tab_mut(state) else {
        return Ok(false);
    };
    tab.scroll_offset = 0;
    match key_code {
        KEY_BACKSPACE => rt::terminal_session_send_input(tab.session_handle, &[0x7f])?,
        KEY_UP => rt::terminal_session_send_input(tab.session_handle, b"\x1b[A")?,
        KEY_DOWN => rt::terminal_session_send_input(tab.session_handle, b"\x1b[B")?,
        KEY_RIGHT => rt::terminal_session_send_input(tab.session_handle, b"\x1b[C")?,
        KEY_LEFT => rt::terminal_session_send_input(tab.session_handle, b"\x1b[D")?,
        _ => return Ok(false),
    }
    Ok(true)
}

fn app_pointer_action_from_word(value: u64) -> Option<rt::AppPointerAction> {
    match value as u32 {
        x if x == rt::AppPointerAction::Down as u32 => Some(rt::AppPointerAction::Down),
        x if x == rt::AppPointerAction::Move as u32 => Some(rt::AppPointerAction::Move),
        x if x == rt::AppPointerAction::Up as u32 => Some(rt::AppPointerAction::Up),
        x if x == rt::AppPointerAction::Scroll as u32 => Some(rt::AppPointerAction::Scroll),
        _ => None,
    }
}

fn handle_pointer_down(state: &mut TerminalState, x: i32, y: i32) -> bool {
    if let Some(tab_index) = crate::render::tab_strip_hit_index(x, y) {
        if tab_index != state.active_tab && state.tabs[tab_index].occupied {
            state.active_tab = tab_index;
            state.selection = None;
            return true;
        }
        return false;
    }
    let Some(cell) = crate::render::pointer_to_cell(state, x, y) else {
        state.selection = None;
        return false;
    };
    state.selection = Some(Selection {
        anchor: cell,
        focus: cell,
        dragging: true,
    });
    true
}

fn handle_pointer_move(state: &mut TerminalState, x: i32, y: i32) -> bool {
    let Some(mut selection) = state.selection else {
        return false;
    };
    if !selection.dragging {
        return false;
    }
    let Some(cell) = crate::render::pointer_to_cell(state, x, y) else {
        return false;
    };
    selection.focus = cell;
    state.selection = Some(selection);
    true
}

fn handle_pointer_up(state: &mut TerminalState, x: i32, y: i32) -> bool {
    let Some(mut selection) = state.selection else {
        return false;
    };
    if let Some(cell) = crate::render::pointer_to_cell(state, x, y) {
        selection.focus = cell;
    }
    selection.dragging = false;
    state.selection = Some(selection);
    crate::render::copy_selection(state);
    true
}

fn handle_pointer_scroll(state: &mut TerminalState, delta_y: i32) {
    let rows = state.rows;
    if let Some(tab) = crate::tabs::active_tab_mut(state) {
        if delta_y > 0 {
            crate::render::scroll_up_view(tab, delta_y as usize, rows);
        } else if delta_y < 0 {
            crate::render::scroll_down_view(tab, (-delta_y) as usize);
        }
    }
}

pub(crate) fn receive_terminal_message(
    session_handle: rt::Handle,
    data: &mut [u8],
) -> rt::Result<Option<TerminalMessage>> {
    match rt::terminal_session_receive_nonblocking(session_handle, data) {
        Ok(Some(len)) => Ok(Some(TerminalMessage::Output(len))),
        Ok(None) => Ok(None),
        Err(rt::Error::NotFound) => Ok(Some(TerminalMessage::Closed)),
        Err(error) => Err(error),
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
