#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

mod control;
mod panes;
mod profiles;
mod render;
mod state;
mod tabs;
mod vt;

use rt::{AppControlTag, ControlTag, RawMessage};
use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;

pub(crate) use state::*;

#[cfg(not(test))]
rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xfa01;
    }
    if startup.tag != ControlTag::Startup as u32
        || startup.handle_count < 3
        || startup.word_count < 4
    {
        return 0xfa02;
    }

    let surface_handle = startup.handles[0];
    let control_handle = startup.handles[1];
    let terminal_handle = startup.handles[2];
    let clipboard_handle = if startup.handle_count > 3 {
        startup.handles[3]
    } else {
        rt::INVALID_HANDLE
    };
    let mut width = startup.words[1] as u32;
    let mut height = startup.words[2] as u32;
    let mut focused = startup.words[3] != 0;

    let mut buffers = match ui::SurfaceBuffers::<SURFACE_BUFFER_SLOTS>::new(
        surface_handle,
        BUFFER_WIDTH,
        BUFFER_HEIGHT,
        BUFFER_WIDTH,
        BUFFER_BYTES,
    ) {
        Ok(buffers) => buffers,
        Err(_) => return 0xfa03,
    };
    let mut presenter = ui::FirstPresentSurface::new(surface_handle);

    tabs::clear_all_grids();
    let mut state = TerminalState {
        width,
        height,
        focused,
        terminal_handle,
        clipboard_handle,
        columns: 0,
        rows: 0,
        content_x: 0,
        content_y: 0,
        content_w: 0,
        content_h: 0,
        active_tab: 0,
        theme_index: 0,
        profile_index: 0,
        tabs: [TerminalTab::empty(); MAX_TABS],
        selection: None,
        clipboard: [0; CLIPBOARD_BYTES],
        clipboard_len: 0,
    };
    tabs::recompute_layout(&mut state);
    // Restore-on-launch: prefer reattaching the most recent detached session
    // (with its retained scrollback) before opening a fresh shell.
    if tabs::restore_most_recent_detached(&mut state).unwrap_or(false) {
        // Reattach path handled; fall through to first render.
    } else if tabs::open_new_tab(&mut state).is_err() {
        return 0xfa05;
    }
    let (slot, buffer) = buffers.current();
    let _ = render::render(&mut presenter, slot, buffer, &state);

    loop {
        let mut did_work = false;
        let mut changed = false;

        match ui::poll_app_lifecycle(bootstrap) {
            Ok(true) => break,
            Ok(false) => {}
            Err(_) => return 0xfa06,
        }

        match control::poll_control(
            control_handle,
            &mut state,
            &mut width,
            &mut height,
            &mut focused,
        ) {
            Ok((control::ControlFlow::Continue, control_changed, control_worked)) => {
                changed |= control_changed;
                did_work |= control_worked;
            }
            Ok((control::ControlFlow::Exit, _, _)) => break,
            Err(_) => return 0xfa07,
        }

        if width != state.width || height != state.height || focused != state.focused {
            state.width = width;
            state.height = height;
            state.focused = focused;
            tabs::recompute_layout(&mut state);
            tabs::refresh_pane_sizes(&mut state);
            changed = true;
        }

        // Drain pending output for every pane of every tab; each pane hosts an
        // independent session whose closure merges or closes its tab.
        let mut data = [0u8; (rt::IPC_MAX_WORDS - 1) * 8];
        'drain: for tab_index in 0..MAX_TABS {
            if !state.tabs[tab_index].occupied {
                continue;
            }
            for pane_index in 0..state.tabs[tab_index].pane_count {
                let Some(session_handle) = state.tabs[tab_index]
                    .panes
                    .get(pane_index)
                    .map(|pane| pane.session_handle)
                    .filter(|handle| *handle != rt::INVALID_HANDLE)
                else {
                    continue;
                };
                let mut output_budget = MAX_OUTPUT_MESSAGES_PER_PANE_PER_TURN;
                loop {
                    match control::receive_terminal_message(session_handle, &mut data) {
                        Ok(Some(control::TerminalMessage::Output(len))) => {
                            let slot = grid_slot(tab_index, pane_index);
                            let pane = &mut state.tabs[tab_index].panes[pane_index];
                            vt::apply_output(pane, slot, &data[..len]);
                            changed = true;
                            did_work = true;
                            output_budget = output_budget.saturating_sub(1);
                            if output_budget == 0 {
                                break;
                            }
                        }
                        Ok(Some(control::TerminalMessage::Closed)) => {
                            close_pane_from_event(&mut state, tab_index, pane_index);
                            changed = true;
                            did_work = true;
                            continue 'drain;
                        }
                        Ok(None) => break,
                        Err(rt::Error::QueueEmpty) => break,
                        Err(_) => return 0xfa08,
                    }
                }
            }
        }

        if changed {
            let (slot, buffer) = buffers.advance();
            let _ = render::render(&mut presenter, slot, buffer, &state);
        }
        if did_work {
            continue;
        }

        if rt::yield_current().is_err() {
            return 0xfa09;
        }
    }

    for tab in state.tabs.iter() {
        if !tab.occupied {
            continue;
        }
        for pane_index in 0..tab.pane_count {
            let handle = tab.panes[pane_index].session_handle;
            if handle != rt::INVALID_HANDLE {
                let _ = rt::terminal_session_close(handle);
                let _ = rt::handle_close(handle);
            }
        }
    }
    0
}

/// A session ended on its own (exit command): merge the split away when the
/// pane was part of one, otherwise close the tab. Keeps at least one tab open.
fn close_pane_from_event(state: &mut TerminalState, tab_index: usize, pane_index: usize) {
    let tab_occupied = state
        .tabs
        .get(tab_index)
        .map(|tab| tab.occupied)
        .unwrap_or(false);
    if !tab_occupied {
        return;
    }
    let had_split = state.tabs[tab_index].tree.split;
    let pane_count = state.tabs[tab_index].pane_count;
    if had_split && pane_count == MAX_PANES_PER_TAB {
        // Merge the split: the surviving pane's data moves to slot 0.
        let focused = pane_index.min(1);
        let handle = state.tabs[tab_index].panes[focused].session_handle;
        if handle != rt::INVALID_HANDLE {
            let _ = rt::terminal_session_close(handle);
            let _ = rt::handle_close(handle);
        }
        if focused == 1 {
            let kept = state.tabs[tab_index].panes[1];
            state.tabs[tab_index].panes[0] = kept;
            crate::panes::move_pane_grid(grid_slot(tab_index, 1), grid_slot(tab_index, 0));
            crate::panes::clear_pane_grid(grid_slot(tab_index, 1));
        } else {
            state.tabs[tab_index].panes[1] = TerminalPane::empty();
            crate::panes::clear_pane_grid(grid_slot(tab_index, 1));
        }
        state.tabs[tab_index].pane_count = 1;
        state.tabs[tab_index].tree.close_split(0);
        state.selection = None;
        tabs::refresh_pane_sizes(state);
        return;
    }
    tabs::close_tab(state, tab_index, crate::tabs::CollapseMode::Kill);
    if tabs::active_tab_count(state) == 0 {
        let _ = tabs::open_new_tab(state);
    }
}
