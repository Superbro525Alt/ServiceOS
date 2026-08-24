#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

mod actions;
mod catalog_meta;
mod control;
mod render;
mod state;

use rt::{ControlTag, RawMessage};
use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;

use crate::actions::{error_label, reload_catalog, set_statusf};
use crate::control::{ControlFlow, poll_control};
use crate::render::render;
use crate::state::{AppState, BUFFER_BYTES, BUFFER_HEIGHT, BUFFER_WIDTH, SURFACE_BUFFER_SLOTS};

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf501;
    }
    if startup.tag != ControlTag::Startup as u32
        || startup.handle_count < 3
        || startup.word_count < 4
    {
        return 0xf502;
    }

    let surface_handle = startup.handles[0];
    let control_handle = startup.handles[1];
    let package_handle = startup.handles[2];
    let mut state = AppState::new(
        startup.words[1] as u32,
        startup.words[2] as u32,
        startup.words[3] != 0,
    );
    let mut buffers = match ui::SurfaceBuffers::<SURFACE_BUFFER_SLOTS>::new(
        surface_handle,
        BUFFER_WIDTH,
        BUFFER_HEIGHT,
        BUFFER_WIDTH,
        BUFFER_BYTES,
    ) {
        Ok(buffers) => buffers,
        Err(_) => return 0xf507,
    };
    let mut presenter = ui::FirstPresentSurface::new(surface_handle);
    let mut startup = ui::DeferredStartup::new();

    set_statusf(&mut state, format_args!("Loading catalog..."));
    let (slot, buffer) = buffers.current();
    if render(&mut presenter, slot, buffer, package_handle, &state).is_err() {
        return 0xf503;
    }

    loop {
        match ui::poll_app_lifecycle(bootstrap) {
            Ok(true) => break,
            Ok(false) => {}
            Err(_) => return 0xf504,
        }

        match poll_control(
            control_handle,
            package_handle,
            &mut buffers,
            &mut presenter,
            &mut state,
        ) {
            Ok(ControlFlow::Idle) => {}
            Ok(ControlFlow::Worked) => continue,
            Ok(ControlFlow::Exit) => break,
            Err(_) => return 0xf505,
        }

        match startup.run(|| match reload_catalog(package_handle, &mut state) {
            Ok(()) => Ok(true),
            Err(error) => {
                set_statusf(
                    &mut state,
                    format_args!("catalog load failed: {}", error_label(error)),
                );
                Ok(true)
            }
        }) {
            Ok(true) => {
                let (slot, buffer) = buffers.advance();
                if render(&mut presenter, slot, buffer, package_handle, &state).is_err() {
                    return 0xf503;
                }
                continue;
            }
            Ok(false) => {}
            Err(_) => return 0xf505,
        }

        if rt::yield_current().is_err() {
            return 0xf506;
        }
    }

    0
}
