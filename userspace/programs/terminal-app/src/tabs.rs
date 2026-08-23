use super::*;

pub(crate) fn recompute_layout(state: &mut TerminalState) {
    let content_width = state.width.saturating_sub((CONTENT_PADDING_X as u32) * 2);
    let content_height = state.height.saturating_sub(
        ui::TITLEBAR_HEIGHT + TAB_STRIP_HEIGHT as u32 + (CONTENT_PADDING_Y as u32) * 2 + 4,
    );
    state.columns = ((content_width as usize) / CELL_WIDTH).clamp(20, MAX_COLS);
    state.rows = ((content_height as usize) / CELL_HEIGHT).clamp(8, MAX_SCROLLBACK_LINES);
    for tab in state.tabs.iter_mut().filter(|tab| tab.occupied) {
        crate::render::clamp_scroll_offset(tab, state.rows);
        if tab.cursor_col >= state.columns {
            tab.cursor_col = state.columns.saturating_sub(1);
        }
    }
}

pub(crate) fn clear_all_tabs() {
    for tab_index in 0..MAX_TABS {
        clear_tab_grid(tab_index);
    }
}

pub(crate) fn clear_tab_grid(tab_index: usize) {
    let lines = unsafe { GRIDS.tab_mut(tab_index) };
    let wraps = unsafe { WRAPS.tab_mut(tab_index) };
    for row in lines.iter_mut() {
        for cell in row.iter_mut() {
            *cell = Cell::blank();
        }
    }
    wraps.fill(false);
}

pub(crate) fn open_new_tab(state: &mut TerminalState) -> rt::Result<()> {
    let Some(index) = state.tabs.iter().position(|tab| !tab.occupied) else {
        return Err(rt::Error::CapacityExceeded);
    };
    let (_, session_handle, _, _) = rt::terminal_session_open(state.terminal_handle)?;
    let _ = rt::terminal_session_resize(
        session_handle,
        state.columns as u32,
        state.rows as u32,
        state.width,
        state.height,
    );
    clear_tab_grid(index);
    state.tabs[index] = TerminalTab {
        occupied: true,
        session_handle,
        session_id: (index + 1) as u32,
        line_count: 1,
        cursor_line: 0,
        cursor_col: 0,
        saved_cursor_line: 0,
        saved_cursor_col: 0,
        scroll_offset: 0,
        parse_state: ParseState::Ground,
        csi_params: [0; 8],
        csi_count: 0,
        csi_private: false,
        osc_bytes: [0; MAX_OSC_BYTES],
        osc_len: 0,
        osc_esc_pending: false,
        title: [0; MAX_TITLE_BYTES],
        title_len: 0,
        current_fg: COLOR_DEFAULT,
        current_bg: COLOR_DEFAULT,
        current_flags: 0,
        cursor_visible: true,
    };
    state.active_tab = index;
    state.selection = None;
    Ok(())
}

pub(crate) fn close_active_tab(state: &mut TerminalState) {
    let active = state.active_tab;
    if active_tab_count(state) <= 1 {
        return;
    }
    close_tab(state, active);
    if !state.tabs[state.active_tab].occupied {
        focus_next_tab(state);
    }
    state.selection = None;
}

pub(crate) fn close_tab(state: &mut TerminalState, tab_index: usize) {
    if tab_index >= MAX_TABS || !state.tabs[tab_index].occupied {
        return;
    }
    let handle = state.tabs[tab_index].session_handle;
    let _ = rt::terminal_session_close(handle);
    let _ = rt::handle_close(handle);
    state.tabs[tab_index] = TerminalTab::empty();
    clear_tab_grid(tab_index);
    if state.active_tab == tab_index {
        focus_next_tab(state);
    }
}

pub(crate) fn active_tab_count(state: &TerminalState) -> usize {
    state.tabs.iter().filter(|tab| tab.occupied).count()
}

pub(crate) fn focus_next_tab(state: &mut TerminalState) {
    for offset in 1..=MAX_TABS {
        let index = (state.active_tab + offset) % MAX_TABS;
        if state.tabs[index].occupied {
            state.active_tab = index;
            state.selection = None;
            return;
        }
    }
}

pub(crate) fn focus_previous_tab(state: &mut TerminalState) {
    for offset in 1..=MAX_TABS {
        let index = (state.active_tab + MAX_TABS - offset) % MAX_TABS;
        if state.tabs[index].occupied {
            state.active_tab = index;
            state.selection = None;
            return;
        }
    }
}

pub(crate) fn active_tab_mut(state: &mut TerminalState) -> Option<&mut TerminalTab> {
    state
        .tabs
        .get_mut(state.active_tab)
        .filter(|tab| tab.occupied)
}

pub(crate) fn active_tab_ref(state: &TerminalState) -> Option<&TerminalTab> {
    state.tabs.get(state.active_tab).filter(|tab| tab.occupied)
}
