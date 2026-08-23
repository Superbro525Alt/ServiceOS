use super::*;

pub(crate) fn recompute_layout(state: &mut TerminalState) {
    let content_width = state.width.saturating_sub((CONTENT_PADDING_X as u32) * 2);
    let content_height = state.height.saturating_sub(
        ui::TITLEBAR_HEIGHT + TAB_STRIP_HEIGHT as u32 + (CONTENT_PADDING_Y as u32) * 2 + 4,
    );
    state.content_x = CONTENT_PADDING_X;
    state.content_y = ui::TITLEBAR_HEIGHT as usize + TAB_STRIP_HEIGHT + CONTENT_PADDING_Y;
    state.content_w = content_width as usize;
    state.content_h = content_height as usize;
    state.columns = ((content_width as usize) / CELL_WIDTH).clamp(20, MAX_COLS);
    state.rows = ((content_height as usize) / CELL_HEIGHT).clamp(8, MAX_SCROLLBACK_LINES);
}

pub(crate) fn clear_all_grids() {
    for slot in 0..GRID_SLOTS {
        crate::panes::clear_pane_grid(slot);
    }
}

pub(crate) fn clear_tab_grid(tab_index: usize) {
    for pane_index in 0..MAX_PANES_PER_TAB {
        crate::panes::clear_pane_grid(grid_slot(tab_index, pane_index));
    }
}

/// Recompute per-pane grid dims from the pane tree and push resizes to
/// terminal-service. Reflows grid contents when a pane's column count changes.
pub(crate) fn refresh_pane_sizes(state: &mut TerminalState) {
    let area = crate::panes::content_area(state);
    for tab_index in 0..MAX_TABS {
        if !state.tabs[tab_index].occupied {
            continue;
        }
        let tree = state.tabs[tab_index].tree;
        let rects = crate::panes::pane_rects(area, &tree);
        let pane_count = state.tabs[tab_index].pane_count;
        for pane_index in 0..pane_count {
            let (cols, rows) = crate::panes::grid_dims_for(rects[pane_index]);
            let slot = grid_slot(tab_index, pane_index);
            let pane = &mut state.tabs[tab_index].panes[pane_index];
            if pane.columns != cols && pane.columns != 0 {
                vt::reflow_pane(pane, slot, pane.columns, cols);
            }
            pane.columns = cols.max(1);
            pane.rows = rows.max(1);
            render::clamp_scroll_offset(pane, rows);
            if pane.cursor_col >= cols {
                pane.cursor_col = cols - 1;
            }
            let _ = rt::terminal_session_resize(
                pane.session_handle,
                pane.columns as u32,
                pane.rows as u32,
                rects[pane_index].w.min(BUFFER_WIDTH as usize) as u32,
                rects[pane_index].h.min(BUFFER_HEIGHT as usize) as u32,
            );
        }
    }
}

pub(crate) fn open_new_tab(state: &mut TerminalState) -> rt::Result<()> {
    let Some(index) = state.tabs.iter().position(|tab| !tab.occupied) else {
        return Err(rt::Error::CapacityExceeded);
    };
    open_tab_with_profile(state, index, state.profile_index)
}

fn open_tab_with_profile(
    state: &mut TerminalState,
    index: usize,
    profile_index: usize,
) -> rt::Result<()> {
    let profile = profiles::DEFAULT_PROFILES[profile_index % profiles::PROFILE_COUNT];
    let (_, session_handle, _, _) =
        profiles::open_session_with_profile(state.terminal_handle, &profile)?;
    clear_tab_grid(index);
    let mut tab = TerminalTab::empty();
    tab.occupied = true;
    tab.profile_index = profile_index % profiles::PROFILE_COUNT;
    tab.pane_count = 1;
    tab.panes[0] = TerminalPane::opened(session_handle, (index + 1) as u32);
    tab.panes[0].columns = state.columns.max(1);
    tab.panes[0].rows = state.rows.max(1);
    state.tabs[index] = tab;
    state.active_tab = index;
    state.selection = None;
    refresh_pane_sizes(state);
    Ok(())
}

/// Split the active tab's focused area along `axis`, hosting an independent
/// session in the new pane with the tab's selected profile.
pub(crate) fn split_active_pane(
    state: &mut TerminalState,
    axis: crate::panes::SplitAxis,
) -> rt::Result<()> {
    let index = state.active_tab;
    if !state.tabs[index].occupied || state.tabs[index].tree.split {
        return Ok(());
    }
    let profile_index = state.tabs[index].profile_index;
    let profile = profiles::DEFAULT_PROFILES[profile_index % profiles::PROFILE_COUNT];
    let (_, session_handle, _, _) =
        profiles::open_session_with_profile(state.terminal_handle, &profile)?;
    let slot = grid_slot(index, 1);
    crate::panes::clear_pane_grid(slot);
    state.tabs[index].panes[1] = TerminalPane::opened(session_handle, 0);
    state.tabs[index].pane_count = 2;
    state.tabs[index].tree.open_split(axis);
    state.selection = None;
    refresh_pane_sizes(state);
    Ok(())
}

/// Close the focused pane; when it is the last pane of the tab, close the tab.
pub(crate) fn close_focused_pane_or_tab(state: &mut TerminalState) {
    let index = state.active_tab;
    if !state.tabs[index].occupied {
        return;
    }
    if state.tabs[index].tree.split {
        let focused = state.tabs[index].tree.focused.min(1);
        let keep = 1 - focused;
        release_pane_session(&mut state.tabs[index], focused);
        // Keep the surviving pane's data; move pane `keep` into slot 0.
        if keep != 0 {
            let kept = state.tabs[index].panes[1];
            state.tabs[index].panes[0] = kept;
            state.tabs[index].panes[1] = TerminalPane::empty();
            crate::panes::move_pane_grid(grid_slot(index, 1), grid_slot(index, 0));
        } else {
            state.tabs[index].panes[1] = TerminalPane::empty();
            crate::panes::clear_pane_grid(grid_slot(index, 1));
        }
        state.tabs[index].pane_count = 1;
        state.tabs[index].tree.close_split(0);
        state.selection = None;
        refresh_pane_sizes(state);
        return;
    }
    close_active_tab(state);
}

fn release_pane_session(tab: &mut TerminalTab, pane_index: usize) {
    let handle = tab.panes[pane_index].session_handle;
    if handle != rt::INVALID_HANDLE {
        let _ = rt::terminal_session_close(handle);
        let _ = rt::handle_close(handle);
    }
    tab.panes[pane_index] = TerminalPane::empty();
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
    let pane_count = state.tabs[tab_index].pane_count;
    for pane_index in 0..pane_count {
        release_pane_session(&mut state.tabs[tab_index], pane_index);
    }
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
