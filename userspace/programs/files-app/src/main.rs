#![no_std]
#![no_main]

mod control;
mod lifecycle;
mod navigation;
mod render;
mod state;

use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;
use rt::{ControlTag, RawMessage};

use crate::control::{poll_control, ControlFlow};
use crate::lifecycle::poll_lifecycle;
use crate::navigation::{reload_directory, reopen_directory};
use crate::render::render;
use crate::state::{ExplorerEntry, ExplorerState, BUFFER_BYTES, BUFFER_HEIGHT, BUFFER_WIDTH, SURFACE_BUFFER_SLOTS};

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf101;
    }
    if startup.tag != ControlTag::Startup as u32
        || startup.handle_count < 3
        || startup.word_count < 4
    {
        return 0xf102;
    }

    let surface_handle = startup.handles[0];
    let control_handle = startup.handles[1];
    let storage_handle = startup.handles[2];
    let mut state = ExplorerState {
        width: startup.words[1] as u32,
        height: startup.words[2] as u32,
        focused: startup.words[3] != 0,
        current_directory_handle: rt::INVALID_HANDLE,
        current_path: [0; state::MAX_STORAGE_PATH],
        current_path_len: 0,
        entries: [ExplorerEntry::empty(); state::MAX_ENTRIES],
        entry_count: 0,
        selected_index: 0,
        scroll_offset: 0,
        load_failed: false,
    };

    let mut buffers = match ui::SurfaceBuffers::<SURFACE_BUFFER_SLOTS>::new(
        surface_handle,
        BUFFER_WIDTH,
        BUFFER_HEIGHT,
        BUFFER_WIDTH,
        BUFFER_BYTES,
    ) {
        Ok(buffers) => buffers,
        Err(_) => return 0xf103,
    };

    let _ = reopen_directory(&mut state, storage_handle);
    let _ = reload_directory(&mut state);
    let (slot, buffer) = buffers.current();
    let _ = render(surface_handle, slot, buffer, &state);

    loop {
        match poll_lifecycle(bootstrap) {
            Ok(true) => break,
            Ok(false) => {}
            Err(_) => return 0xf105,
        }

        match poll_control(
            control_handle,
            surface_handle,
            &mut buffers,
            storage_handle,
            &mut state,
        ) {
            Ok(ControlFlow::Idle) => {}
            Ok(ControlFlow::Worked) => continue,
            Ok(ControlFlow::Exit) => break,
            Err(_) => return 0xf106,
        }

        if rt::yield_current().is_err() {
            return 0xf107;
        }
    }

    if state.current_directory_handle != rt::INVALID_HANDLE {
        let _ = rt::handle_close(state.current_directory_handle);
    }
    0
}
