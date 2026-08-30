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
            Ok(())
                if message.tag == AppControlTag::FocusChanged as u32 && message.word_count > 0 =>
            {
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
                let action = ui::decode_app_pointer_action(message.words[0]);
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
                did_work = true;
                if let Some(ch) = core::char::from_u32(message.words[0] as u32) {
                    // Active Ctrl-R search swallows typed text first; Enter
                    // accepts the match and executes it.
                    if crate::search::handle_text(state, ch)? {
                        changed = true;
                        continue;
                    }
                    let mut visual_changed = state.selection.is_some();
                    state.selection = None;
                    let session_handle = {
                        let Some(tab) = crate::tabs::active_tab_mut(state) else {
                            continue;
                        };
                        let Some(pane) = tab.focused_pane_mut() else {
                            continue;
                        };
                        visual_changed |= pane.scroll_offset != 0;
                        pane.scroll_offset = 0;
                        // Keep the local line mirror in lockstep with what we
                        // forward so Ctrl-R searches this pane's commands.
                        if ch == '\n' || ch == '\r' {
                            crate::search::commit_line(pane);
                        } else if !ch.is_control() {
                            crate::search::note_char(pane, ch);
                        }
                        pane.session_handle
                    };
                    let mut bytes = [0u8; 4];
                    let encoded = ch.encode_utf8(&mut bytes);
                    let _ = rt::terminal_session_send_input(session_handle, encoded.as_bytes());
                    changed |= visual_changed;
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
            // Close focused pane first; when the tab has no split left it
            // closes the whole tab.
            KEY_W => {
                crate::tabs::close_focused_pane_or_tab(state);
                return Ok(true);
            }
            KEY_E => {
                crate::tabs::split_active_pane(state, crate::panes::SplitAxis::Columns)?;
                return Ok(true);
            }
            KEY_D => {
                crate::tabs::split_active_pane(state, crate::panes::SplitAxis::Rows)?;
                return Ok(true);
            }
            // Detach: close the pane but keep the session (scrollback, line
            // state, bookmarks) alive in terminal-service for reattach.
            KEY_Q => {
                crate::tabs::detach_focused_pane_or_tab(state);
                return Ok(true);
            }
            // Cycle command bookmarks into the input line (re-edit only,
            // never auto-executed).
            KEY_B => {
                send_session_op(state, crate::wire::SESSION_BOOKMARK_CYCLE)?;
                return Ok(true);
            }
            KEY_P => {
                state.profile_index = (state.profile_index + 1) % profiles::PROFILE_COUNT;
                return Ok(true);
            }
            // Ctrl-R: start (or cycle older) reverse incremental history
            // search on the focused pane.
            KEY_R => {
                let _ = crate::search::begin_or_cycle(state);
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
            // Cycle to the next named theme, wrapping past the registry end.
            KEY_Y => {
                set_theme(state, crate::state::next_theme_index(state.theme_index));
                return Ok(true);
            }
            KEY_1 => {
                set_theme(state, 0);
                return Ok(true);
            }
            KEY_2 => {
                set_theme(state, 1);
                return Ok(true);
            }
            KEY_3 => {
                set_theme(state, 2);
                return Ok(true);
            }
            KEY_4 => {
                set_theme(state, 3);
                return Ok(true);
            }
            KEY_5 => {
                set_theme(state, 4);
                return Ok(true);
            }
            KEY_6 => {
                set_theme(state, 5);
                return Ok(true);
            }
            _ => {}
        }
    }

    // Pane focus navigation and split-ratio resize.
    if modifiers & MOD_CTRL != 0 && modifiers & MOD_ALT != 0 {
        use crate::panes::PaneDirection;
        let direction = match key_code {
            KEY_LEFT => Some(PaneDirection::Left),
            KEY_RIGHT => Some(PaneDirection::Right),
            KEY_UP => Some(PaneDirection::Up),
            KEY_DOWN => Some(PaneDirection::Down),
            _ => None,
        };
        if let Some(direction) = direction {
            if let Some(delta) = crate::panes::pane_resize_delta(key_code, modifiers) {
                let mut resized = false;
                if let Some(tab) = crate::tabs::active_tab_mut(state) {
                    if tab.tree.split {
                        tab.tree.resize_ratio(delta);
                        resized = true;
                    }
                }
                // The ratio change moves the pane rects immediately:
                // re-derive grid sizes, reflow scrollback, and push the new
                // geometry to terminal-service now instead of waiting for
                // the next window-geometry event.
                if resized {
                    crate::tabs::refresh_pane_sizes(state);
                }
            } else if let Some(tab) = crate::tabs::active_tab_mut(state) {
                tab.tree.focus_direction(direction);
            }
            return Ok(true);
        }
    }

    // Esc leaves an active search untouched; Ctrl-R cycles older matches.
    if key_code == KEY_ESC && crate::search::cancel(state) {
        return Ok(true);
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
            if let Some(pane) = tab.focused_pane_mut() {
                pane.scroll_offset = 0;
                rt::terminal_session_send_input(pane.session_handle, &[0x03])?;
                return Ok(true);
            }
        }
    }
    // Bookmark the current command line as typed (service snapshots it).
    if modifiers & MOD_CTRL != 0 && key_code == KEY_B {
        send_session_op(state, crate::wire::SESSION_BOOKMARK_ADD)?;
        return Ok(true);
    }
    if key_code == KEY_PAGE_UP || (modifiers & MOD_SHIFT != 0 && key_code == KEY_UP) {
        let state_rows = state.rows;
        if let Some(tab) = crate::tabs::active_tab_mut(state) {
            let rows = tab
                .focused_pane_ref()
                .map(|pane| pane.rows)
                .unwrap_or(state_rows);
            if let Some(pane) = tab.focused_pane_mut() {
                crate::render::scroll_up_view(
                    pane,
                    if key_code == KEY_PAGE_UP {
                        rows.saturating_sub(1).max(1)
                    } else {
                        1
                    },
                    rows,
                );
                return Ok(true);
            }
        }
    }
    if key_code == KEY_PAGE_DOWN || (modifiers & MOD_SHIFT != 0 && key_code == KEY_DOWN) {
        if let Some(tab) = crate::tabs::active_tab_mut(state) {
            if let Some(pane) = tab.focused_pane_mut() {
                crate::render::scroll_down_view(
                    pane,
                    if key_code == KEY_PAGE_DOWN {
                        pane.rows.saturating_sub(1).max(1)
                    } else {
                        1
                    },
                );
                return Ok(true);
            }
        }
    }

    // While a Ctrl-R search is active on this pane, backspace edits the query.
    if key_code == KEY_BACKSPACE && state.search.is_some() {
        if crate::search::handle_backspace(state) {
            return Ok(true);
        }
    }
    state.selection = None;
    let Some(tab) = crate::tabs::active_tab_mut(state) else {
        return Ok(false);
    };
    let Some(pane) = tab.focused_pane_mut() else {
        return Ok(false);
    };
    let visual_changed = pane.scroll_offset != 0;
    pane.scroll_offset = 0;
    match key_code {
        KEY_BACKSPACE => {
            crate::search::note_backspace(pane);
            rt::terminal_session_send_input(pane.session_handle, &[0x7f])?
        }
        KEY_UP => {
            crate::search::history_up(pane);
            rt::terminal_session_send_input(pane.session_handle, b"\x1b[A")?
        }
        KEY_DOWN => {
            crate::search::history_down(pane);
            rt::terminal_session_send_input(pane.session_handle, b"\x1b[B")?
        }
        KEY_RIGHT => rt::terminal_session_send_input(pane.session_handle, b"\x1b[C")?,
        KEY_LEFT => rt::terminal_session_send_input(pane.session_handle, b"\x1b[D")?,
        _ => return Ok(false),
    }
    Ok(visual_changed)
}

/// Apply a theme pick: recolor now, fold it into the active profile's stored
/// theme, persist the profile set durably (best effort), and mirror the pick
/// to terminal-service so the session's service-side override and the
/// service-global active theme track the operator's latest choice. The
/// durable copy is written first: persistence happens before the pick is
/// mirrored or repainted, matching the access.cfg degrade model (the mirror
/// is best-effort and the service keeps theme state in memory only).
fn set_theme(state: &mut TerminalState, theme_index: usize) {
    state.theme_index = theme_index % crate::THEMES.len();
    let profile_index = state.profile_index % profiles::PROFILE_COUNT;
    state.profiles[profile_index].theme_index = state.theme_index as u8;
    if state.storage_handle != rt::INVALID_HANDLE {
        let _ = profiles::store_profiles(state.storage_handle, &state.profiles);
    }
    send_theme_set(state);
}

/// Mirror the active theme pick to terminal-service on the focused pane's
/// session channel. Best effort: an unavailable session simply keeps the
/// app-local theme.
fn send_theme_set(state: &TerminalState) {
    let handle = crate::tabs::active_tab_ref(state)
        .and_then(|tab| tab.focused_pane_ref())
        .map(|pane| pane.session_handle)
        .filter(|handle| *handle != rt::INVALID_HANDLE);
    let Some(handle) = handle else {
        return;
    };
    let mut message = rt::RawMessage::empty(crate::wire::THEME_SET);
    message.word_count = 1;
    message.words[0] = state.theme_index as u64;
    let _ = rt::channel_send(handle, &message);
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
    let Some((pane_index, cell)) = crate::render::pointer_to_cell(state, x, y) else {
        state.selection = None;
        return false;
    };
    // Focus follows click.
    let focus_changed = state
        .tabs
        .get(state.active_tab)
        .map(|tab| tab.tree.focused != pane_index)
        .unwrap_or(false);
    if let Some(tab) = crate::tabs::active_tab_mut(state) {
        tab.tree.focused = pane_index.min(MAX_PANES_PER_TAB - 1);
    }
    state.selection = Some(Selection {
        pane: pane_index,
        anchor: cell,
        focus: cell,
        dragging: true,
    });
    focus_changed
}

fn handle_pointer_move(state: &mut TerminalState, x: i32, y: i32) -> bool {
    let Some(mut selection) = state.selection else {
        return false;
    };
    if !selection.dragging {
        return false;
    }
    let Some((pane_index, cell)) = crate::render::pointer_to_cell(state, x, y) else {
        return false;
    };
    if pane_index != selection.pane {
        return false;
    }
    selection.focus = cell;
    state.selection = Some(selection);
    true
}

fn handle_pointer_up(state: &mut TerminalState, x: i32, y: i32) -> bool {
    let Some(mut selection) = state.selection else {
        return false;
    };
    if let Some((pane_index, cell)) = crate::render::pointer_to_cell(state, x, y) {
        if pane_index == selection.pane {
            selection.focus = cell;
        }
    }
    selection.dragging = false;
    state.selection = Some(selection);
    crate::render::copy_selection(state);
    true
}

fn handle_pointer_scroll(state: &mut TerminalState, delta_y: i32) {
    let state_rows = state.rows;
    if let Some(tab) = crate::tabs::active_tab_mut(state) {
        let rows = tab
            .focused_pane_ref()
            .map(|pane| pane.rows)
            .unwrap_or(state_rows);
        if let Some(pane) = tab.focused_pane_mut() {
            if delta_y > 0 {
                crate::render::scroll_up_view(pane, delta_y as usize, rows);
            } else if delta_y < 0 {
                crate::render::scroll_down_view(pane, (-delta_y) as usize);
            }
        }
    }
}

/// Fire a session-channel op (detach handled in tabs; bookmark add/cycle
/// here) on the focused pane's session.
fn send_session_op(state: &mut TerminalState, tag: u32) -> rt::Result<()> {
    let handle = crate::tabs::active_tab_mut(state)
        .and_then(|tab| tab.focused_pane_ref())
        .map(|pane| pane.session_handle)
        .filter(|handle| *handle != rt::INVALID_HANDLE);
    match handle {
        Some(handle) => rt::channel_send(handle, &rt::RawMessage::empty(tag)),
        None => Ok(()),
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
