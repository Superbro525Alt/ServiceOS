#![no_std]
#![no_main]

mod actions;
mod control;
mod lifecycle;
mod render;
mod state;

use core::array;

use serviceos_userspace_runtime as rt;
use rt::{ControlTag, RawMessage};

use crate::actions::reload_catalog;
use crate::control::{poll_control, ControlFlow};
use crate::lifecycle::poll_lifecycle;
use crate::render::render;
use crate::state::{AppState, CatalogEntry, BUFFER_BYTES, BUFFER_HEIGHT, BUFFER_WIDTH, MAX_ENTRIES, MAX_STATUS_BYTES, SURFACE_BUFFER_SLOTS};

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf501;
    }
    if startup.tag != ControlTag::Startup as u32 || startup.handle_count < 3 || startup.word_count < 4 {
        return 0xf502;
    }

    let surface_handle = startup.handles[0];
    let control_handle = startup.handles[1];
    let package_handle = startup.handles[2];
    let mut state = AppState {
        width: startup.words[1] as u32,
        height: startup.words[2] as u32,
        focused: startup.words[3] != 0,
        entries: [CatalogEntry::empty(); MAX_ENTRIES],
        entry_count: 0,
        selected_index: 0,
        scroll_offset: 0,
        status: [0; MAX_STATUS_BYTES],
        status_len: 0,
    };
    let mut buffer_handles = [rt::INVALID_HANDLE; SURFACE_BUFFER_SLOTS];
    let mut mapped_buffers: [Option<rt::MappedMemory>; SURFACE_BUFFER_SLOTS] =
        array::from_fn(|_| None);
    for slot in 0..SURFACE_BUFFER_SLOTS {
        let buffer_handle = match rt::memory_create(BUFFER_BYTES, true) {
            Ok(handle) => handle,
            Err(_) => return 0xf507,
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
            return 0xf508;
        }
        let mapped_buffer = match rt::MappedMemory::map(buffer_handle, BUFFER_BYTES, true) {
            Ok(buffer) => buffer,
            Err(_) => {
                let _ = rt::handle_close(buffer_handle);
                return 0xf509;
            }
        };
        buffer_handles[slot] = buffer_handle;
        mapped_buffers[slot] = Some(mapped_buffer);
    }
    let mut front_buffer_slot = 0usize;

    let _ = reload_catalog(package_handle, &mut state);
    if render(
        surface_handle,
        front_buffer_slot as u32,
        mapped_buffers[front_buffer_slot].as_mut().unwrap(),
        package_handle,
        &state,
    )
    .is_err()
    {
        return 0xf503;
    }

    loop {
        match poll_lifecycle(bootstrap) {
            Ok(true) => break,
            Ok(false) => {}
            Err(_) => return 0xf504,
        }

        match poll_control(
            control_handle,
            surface_handle,
            package_handle,
            &mut mapped_buffers,
            &mut front_buffer_slot,
            &mut state,
        ) {
            Ok(ControlFlow::Idle) => {}
            Ok(ControlFlow::Worked) => continue,
            Ok(ControlFlow::Exit) => break,
            Err(_) => return 0xf505,
        }

        if rt::yield_current().is_err() {
            return 0xf506;
        }
    }

    for handle in buffer_handles {
        if handle != rt::INVALID_HANDLE {
            let _ = rt::handle_close(handle);
        }
    }
    0
}
