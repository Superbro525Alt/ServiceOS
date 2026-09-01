#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

mod audioclient;
mod codec;
mod control;
mod library;
mod plan;
mod render;
mod state;
mod wav;

use rt::{ControlTag, RawMessage};
use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;

use crate::control::{poll_control, pump_playback, stop_playback};
use crate::render::render;
use crate::state::{BUFFER_BYTES, BUFFER_HEIGHT, BUFFER_WIDTH, MediaState, SURFACE_BUFFER_SLOTS};

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf201;
    }
    if startup.tag != ControlTag::Startup as u32
        || startup.handle_count < 3
        || startup.word_count < 4
    {
        return 0xf202;
    }

    let surface_handle = startup.handles[0];
    let control_handle = startup.handles[1];
    let storage_handle = startup.handles[2];
    let desktop_handle =
        rt::lookup_service(bootstrap, rt::ServiceId::DesktopShell).unwrap_or(rt::INVALID_HANDLE);
    let audio_handle =
        rt::lookup_service(bootstrap, rt::ServiceId::Audio).unwrap_or(rt::INVALID_HANDLE);
    let mut state = MediaState::new(
        startup.words[1] as u32,
        startup.words[2] as u32,
        startup.words[3] != 0,
    );
    state.desktop_handle = desktop_handle;

    let mut buffers = match ui::SurfaceBuffers::<SURFACE_BUFFER_SLOTS>::new(
        surface_handle,
        BUFFER_WIDTH,
        BUFFER_HEIGHT,
        BUFFER_WIDTH,
        BUFFER_BYTES,
    ) {
        Ok(buffers) => buffers,
        Err(_) => return 0xf203,
    };
    let mut presenter = ui::FirstPresentSurface::new(surface_handle);
    let mut deferred = ui::DeferredStartup::new();
    let (slot, buffer) = buffers.current();
    let _ = render(&mut presenter, slot, buffer, &state);
    loop {
        match ui::poll_app_lifecycle(bootstrap) {
            Ok(true) => break,
            Ok(false) => {}
            Err(_) => return 0xf205,
        }

        let mut changed = false;
        match poll_control(control_handle, &mut state, storage_handle, audio_handle) {
            Ok(flow_changed) => changed |= flow_changed,
            Err(_) => return 0xf206,
        }
        changed |= pump_playback(&mut state, storage_handle, audio_handle);

        match deferred.run(|| {
            if !state.scan_done {
                library::scan_library(storage_handle, &mut state);
                return Ok(true);
            }
            Ok(false)
        }) {
            Ok(true) => changed = true,
            Ok(false) => {}
            Err(_) => return 0xf206,
        }

        if changed {
            let (slot, buffer) = buffers.advance();
            let _ = render(&mut presenter, slot, buffer, &state);
        }

        if rt::yield_current().is_err() {
            return 0xf207;
        }
    }

    stop_playback(&mut state, "MEDIA stopped");
    0
}
