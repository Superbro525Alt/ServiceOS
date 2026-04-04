#![no_std]
#![no_main]

use core::array;

mod control;
mod render;
mod state;
mod tabs;
mod vt;

use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;
use rt::{AppControlTag, ControlTag, LifecycleEvent, RawMessage};

pub(crate) use state::*;

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xfa01;
    }
    if startup.tag != ControlTag::Startup as u32 || startup.handle_count < 3 || startup.word_count < 4 {
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

    let mut buffer_handles = [rt::INVALID_HANDLE; SURFACE_BUFFER_SLOTS];
    let mut mapped_buffers: [Option<rt::MappedMemory>; SURFACE_BUFFER_SLOTS] =
        array::from_fn(|_| None);
    for slot in 0..SURFACE_BUFFER_SLOTS {
        let buffer_handle = match rt::memory_create(BUFFER_BYTES, true) {
            Ok(handle) => handle,
            Err(_) => return 0xfa03,
        };
        if rt::surface_attach_buffer_slot(
            surface_handle,
            slot as u32,
            buffer_handle,
            BUFFER_WIDTH,
            BUFFER_HEIGHT,
            BUFFER_WIDTH,
        )
        .is_err()
        {
            let _ = rt::handle_close(buffer_handle);
            return 0xfa04;
        }
        let mapped_buffer = match rt::MappedMemory::map(buffer_handle, BUFFER_BYTES, true) {
            Ok(buffer) => buffer,
            Err(_) => {
                let _ = rt::handle_close(buffer_handle);
                return 0xfa0a;
            }
        };
        buffer_handles[slot] = buffer_handle;
        mapped_buffers[slot] = Some(mapped_buffer);
    }
    let mut front_buffer_slot = 0usize;

    tabs::clear_all_tabs();
    let mut state = TerminalState {
        width,
        height,
        focused,
        terminal_handle,
        clipboard_handle,
        columns: 0,
        rows: 0,
        active_tab: 0,
        theme_index: 0,
        tabs: [TerminalTab::empty(); MAX_TABS],
        selection: None,
        clipboard: [0; CLIPBOARD_BYTES],
        clipboard_len: 0,
    };
    tabs::recompute_layout(&mut state);
    if tabs::open_new_tab(&mut state).is_err() {
        return 0xfa05;
    }
    let _ = render::render(
        surface_handle,
        front_buffer_slot as u32,
        mapped_buffers[front_buffer_slot].as_mut().unwrap(),
        &state,
    );

    loop {
        let mut did_work = false;
        let mut changed = false;

        match control::poll_lifecycle(bootstrap) {
            Ok(true) => break,
            Ok(false) => {}
            Err(_) => return 0xfa06,
        }

        match control::poll_control(control_handle, &mut state, &mut width, &mut height, &mut focused) {
            Ok((control::ControlFlow::Continue, control_changed, control_worked)) => {
                changed |= control_changed;
                did_work |= control_worked;
            }
            Ok((control::ControlFlow::Exit, _, _)) => break,
            Err(_) => return 0xfa07,
        }

        if width != state.width || height != state.height || focused != state.focused {
            let old_width = state.width;
            let old_height = state.height;
            let old_columns = state.columns;
            let old_rows = state.rows;
            state.width = width;
            state.height = height;
            state.focused = focused;
            tabs::recompute_layout(&mut state);
            if state.columns != old_columns {
                for tab_index in 0..MAX_TABS {
                    if state.tabs[tab_index].occupied {
                        vt::reflow_tab(&mut state.tabs[tab_index], tab_index, old_columns, state.columns);
                    }
                }
            }
            if state.columns != old_columns
                || state.rows != old_rows
                || state.width != old_width
                || state.height != old_height
            {
                for tab in state.tabs.iter().copied().filter(|tab| tab.occupied) {
                    let _ = rt::terminal_session_resize(
                        tab.session_handle,
                        state.columns as u32,
                        state.rows as u32,
                        state.width,
                        state.height,
                    );
                }
            }
            changed = true;
        }

        let mut data = [0u8; (rt::IPC_MAX_WORDS - 1) * 8];
        for tab_index in 0..MAX_TABS {
            if !state.tabs[tab_index].occupied {
                continue;
            }
            loop {
                match control::receive_terminal_message(state.tabs[tab_index].session_handle, &mut data) {
                    Ok(Some(control::TerminalMessage::Output(len))) => {
                        vt::apply_output(&mut state, tab_index, &data[..len]);
                        changed = true;
                        did_work = true;
                    }
                    Ok(Some(control::TerminalMessage::Closed)) => {
                        tabs::close_tab(&mut state, tab_index);
                        changed = true;
                        did_work = true;
                        if tabs::active_tab_count(&state) == 0 {
                            let _ = tabs::open_new_tab(&mut state);
                        }
                        break;
                    }
                    Ok(None) => break,
                    Err(rt::Error::QueueEmpty) => break,
                    Err(_) => return 0xfa08,
                }
            }
        }

        if changed {
            front_buffer_slot = (front_buffer_slot + 1) % SURFACE_BUFFER_SLOTS;
            let _ = render::render(
                surface_handle,
                front_buffer_slot as u32,
                mapped_buffers[front_buffer_slot].as_mut().unwrap(),
                &state,
            );
        }
        if did_work {
            continue;
        }

        if rt::yield_current().is_err() {
            return 0xfa09;
        }
    }

    for tab in state.tabs.iter().copied().filter(|tab| tab.occupied) {
        let _ = rt::terminal_session_close(tab.session_handle);
        let _ = rt::handle_close(tab.session_handle);
    }
    for handle in buffer_handles {
        if handle != rt::INVALID_HANDLE {
            let _ = rt::handle_close(handle);
        }
    }
    0
}
