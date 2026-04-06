#![no_std]
#![no_main]

mod compose;
mod logging;
mod requests;
mod types;

use serviceos_userspace_runtime as rt;
use rt::{ControlTag, LogEvent, LogSeverity, RawMessage, ServiceId};

use crate::{
    compose::{compose_and_present, compose_damage_and_present, cursor_present},
    logging::{emit_log, poll_lifecycle},
    requests::{drain_public_requests, drain_surface_requests},
    types::{
        CURSOR_PRESENT_COALESCE_TICKS, DirtyState, MAX_FRAMEBUFFER_BYTES, MAX_SURFACES,
        PRESENT_COALESCE_TICKS, SurfaceSlot, active_surface_count,
    },
};

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xfc01;
    }
    if startup.tag != ControlTag::Startup as u32 || startup.handle_count < 2 {
        return 0xfc02;
    }

    let output_handle = startup.handles[0];
    let log_handle = startup.handles[1];
    let output = match rt::display_output_info(output_handle) {
        Ok(info) => info,
        Err(_) => return 0xfc03,
    };
    if output.bytes_per_pixel != 4 || output.byte_len as usize > MAX_FRAMEBUFFER_BYTES {
        return 0xfc04;
    }

    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return 0xfc05,
    };
    if rt::register_service(bootstrap, ServiceId::Graphics, public.second).is_err() {
        return 0xfc06;
    }
    let _ = rt::handle_close(public.second);

    let _ = emit_log(
        log_handle,
        LogSeverity::Info,
        LogEvent::DisplayOutputReady,
        output.width as u64,
        output.height as u64,
    );

    let mut surfaces = [SurfaceSlot::empty(); MAX_SURFACES];
    let mut next_surface_id = 1u32;
    let mut present_count = 0u64;
    let mut last_logged_surface_count = 0usize;
    let mut dirty = DirtyState::Full { immediate: true };
    let mut present_deadline = 0u64;

    loop {
        match poll_lifecycle(bootstrap) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(_) => return 0xfc07,
        }

        let had_public_work = match drain_public_requests(
            public.first,
            log_handle,
            output,
            present_count,
            &mut surfaces,
            &mut next_surface_id,
            &mut dirty,
        ) {
            Ok(had_work) => had_work,
            Err(_) => return 0xfc08,
        };
        let had_surface_work = match drain_surface_requests(&mut surfaces, &mut dirty) {
            Ok(had_work) => had_work,
            Err(_) => return 0xfc0a,
        };
        let _had_work = had_public_work || had_surface_work;

        if !matches!(dirty, DirtyState::Clean) {
            let now = rt::monotonic_now().unwrap_or(0);
            if present_deadline == 0 {
                present_deadline = now.saturating_add(match dirty {
                    DirtyState::CursorOnly(_) => CURSOR_PRESENT_COALESCE_TICKS,
                    _ => PRESENT_COALESCE_TICKS,
                });
            }
            let should_present = match dirty {
                DirtyState::Clean => false,
                DirtyState::CursorOnly(_) => now >= present_deadline,
                DirtyState::Region { immediate, .. } => immediate || now >= present_deadline,
                DirtyState::Full { immediate } => immediate || now >= present_deadline,
            };
            if should_present {
                let result = match dirty {
                    DirtyState::CursorOnly(damage) => {
                        cursor_present(output_handle, output, &surfaces, damage)
                    }
                    DirtyState::Region { damage, .. } => {
                        compose_damage_and_present(output_handle, output, &surfaces, damage)
                    }
                    DirtyState::Full { .. } => compose_and_present(output_handle, output, &surfaces),
                    DirtyState::Clean => Ok(()),
                };
                if result.is_err() {
                    return 0xfc0b;
                }
                present_count = present_count.saturating_add(1);
                let surface_count = active_surface_count(&surfaces);
                if present_count == 1 || surface_count != last_logged_surface_count {
                    let _ = emit_log(
                        log_handle,
                        LogSeverity::Info,
                        LogEvent::CompositorPresented,
                        surface_count as u64,
                        present_count,
                    );
                    last_logged_surface_count = surface_count;
                }
                dirty = DirtyState::Clean;
                present_deadline = 0;
            }
        } else {
            present_deadline = 0;
        }

        if rt::yield_current().is_err() {
            return 0xfc0c;
        }
    }
}
