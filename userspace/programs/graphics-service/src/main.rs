#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

mod compose;
mod e2e_probe;
mod fence;
mod logging;
mod outputs;
mod requests;
mod types;

use rt::{ControlTag, LogEvent, LogSeverity, RawMessage, ServiceId};
use serviceos_userspace_runtime as rt;

use crate::{
    compose::{
        compose_and_present, compose_damage_and_present, cursor_present, presented_frame_slice,
    },
    fence::{FenceTracker, FenceWaiters},
    logging::{emit_log, poll_lifecycle},
    requests::{
        drain_public_requests, drain_surface_requests, flush_close_pending_surfaces,
        handle_public_request, release_fence_waiters,
    },
    types::{
        CURSOR_PRESENT_COALESCE_TICKS, DirtyState, MAX_FRAMEBUFFER_BYTES, MAX_SURFACES,
        PRESENT_COALESCE_TICKS, PresentStats, SurfaceSlot, active_surface_count,
    },
};

rt::entry!(main);

const IDLE_WAIT_TICKS: u64 = 2;

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
    let mut registry = outputs::OutputRegistry::new();
    if registry.register_primary(output_handle, output).is_none() {
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
    let _ = rt::write_log(
        "graphics",
        "present-fence v0: PresentBufferReply word1 carries frame-counter token; \
         completion query = output-status word12 >= token; noop-skip/saved-bytes/close-pending stats in words 13..=15",
    );
    let _ = rt::write_log(
        "graphics",
        "fence-wait v1: op 0x912 (token, timeout_ticks) blocks clients on the \
         completed high-water via bounded waiter reap; reply 0x913 = status + completed. \
         Partial flush: full-frame and damage presents diff against the presented shadow \
         and flush only changed scanline bands when the changed area is a strict subset \
         (<50% of frame), logging 'partial-flush savings' lines with bytes not copied.",
    );

    let mut surfaces = [SurfaceSlot::empty(); MAX_SURFACES];
    let mut next_surface_id = 1u32;
    let mut present_count = 0u64;
    let mut last_logged_surface_count = 0usize;
    let mut dirty = DirtyState::Full { immediate: true };
    let mut present_deadline = 0u64;
    let mut stats = PresentStats::default();
    let mut fences = FenceTracker::new();
    let mut waiters = FenceWaiters::new();
    let mut e2e_probe = e2e_probe::GfxProbe::new();

    loop {
        match poll_lifecycle(bootstrap) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(_) => return 0xfc07,
        }
        // Expire fence waits whose deadlines passed without a covering present.
        release_fence_waiters(&mut waiters, fences.completed());

        let had_public_work = match drain_public_requests(
            public.first,
            log_handle,
            &mut registry,
            fences.completed(),
            &mut waiters,
            &mut surfaces,
            &mut next_surface_id,
            &mut dirty,
        ) {
            Ok(had_work) => had_work,
            Err(_) => return 0xfc08,
        };
        let had_surface_work =
            match drain_surface_requests(&mut surfaces, &mut dirty, present_count) {
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
                let byte_len = output.byte_len as usize;
                let presented = presented_frame_slice(byte_len);
                let allow_noop_skip = present_count > 0;
                let fence = fences.issue();
                let result = match dirty {
                    DirtyState::CursorOnly(damage) => cursor_present(
                        output_handle,
                        output,
                        &surfaces,
                        damage,
                        presented,
                        allow_noop_skip,
                    ),
                    DirtyState::Region { damages, .. } => {
                        let mut result = Ok(compose::PresentOutcome::presented());
                        for index in 0..damages.len {
                            result = compose_damage_and_present(
                                output_handle,
                                output,
                                &surfaces,
                                damages.rects[index],
                                presented,
                                allow_noop_skip,
                            );
                            if result.is_err() {
                                break;
                            }
                        }
                        result
                    }
                    DirtyState::Full { .. } => compose_and_present(
                        output_handle,
                        output,
                        &surfaces,
                        presented,
                        allow_noop_skip,
                    ),
                    DirtyState::Clean => Ok(compose::PresentOutcome::presented()),
                };
                let primary_damage_hint = match dirty {
                    DirtyState::CursorOnly(damage) => Some(damage),
                    DirtyState::Region { damages, .. } => Some(damages.bounding_rect()),
                    DirtyState::Full { .. } | DirtyState::Clean => None,
                };
                match result {
                    Ok(outcome) => {
                        let skips_before = stats.noop_skips;
                        let bands_before = stats.band_presents;
                        let band_saved = outcome.band_saved_bytes;
                        stats.record(&outcome);
                        if let Some(slot) = registry.primary_mut() {
                            slot.record_outcome(&outcome);
                        }
                        fences.complete(fence);
                        release_fence_waiters(&mut waiters, fences.completed());
                        if band_saved > 0 && (bands_before == 0 || bands_before % 64 == 63) {
                            let _ = rt::write_logf(
                                "graphics",
                                format_args!(
                                    "partial-flush savings: event={} saved_bytes={} total_band_events={} total_band_saved_bytes={} present_count={}",
                                    bands_before + 1,
                                    band_saved,
                                    bands_before + 1,
                                    stats.band_saved_bytes,
                                    present_count
                                ),
                            );
                        }
                        if outcome.skipped && skips_before == 0 {
                            let _ = rt::write_logf(
                                "graphics",
                                format_args!(
                                    "partial-present noop skip: saved_bytes={} skips={} present_count={}",
                                    outcome.saved_bytes, stats.noop_skips, present_count
                                ),
                            );
                        } else if present_count == 0 {
                            let _ = rt::write_logf(
                                "graphics",
                                format_args!(
                                    "present-fence v0: token={} completed={} stats(skips={},saved_bytes={})",
                                    fence,
                                    fences.completed(),
                                    stats.noop_skips,
                                    stats.noop_saved_bytes
                                ),
                            );
                        }
                    }
                    Err(_) => return 0xfc0b,
                }
                present_count = present_count.saturating_add(1);
                e2e_probe.note_present(present_count, fences.completed());
                outputs::refresh_virtual_mirrors(&mut registry, primary_damage_hint);
                let _ = flush_close_pending_surfaces(&mut surfaces, &mut dirty);
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

        let mut wait_ticks = match dirty {
            DirtyState::Clean => IDLE_WAIT_TICKS,
            _ => present_deadline
                .saturating_sub(rt::monotonic_now().unwrap_or(0))
                .max(1),
        };
        // Wake in time to expire parked fence waits at their deadlines.
        if let Some(deadline) = waiters.earliest_deadline() {
            let until_deadline = deadline
                .saturating_sub(rt::monotonic_now().unwrap_or(0))
                .max(1);
            wait_ticks = wait_ticks.min(until_deadline);
        }
        let mut waited = RawMessage::empty(0);
        match rt::channel_receive_blocking_timeout(public.first, &mut waited, wait_ticks) {
            Ok(()) => {
                if handle_public_request(
                    &waited,
                    log_handle,
                    &mut registry,
                    fences.completed(),
                    &mut waiters,
                    &mut surfaces,
                    &mut next_surface_id,
                    &mut dirty,
                )
                .is_err()
                {
                    return 0xfc08;
                }
            }
                Err(rt::Error::QueueEmpty) => {
                    // E2E gated synthetic cursor-band cycles exercise the
                    // partial-present planner on idle boots (no monotonic
                    // clock dependency); inert without SERVICEOS_E2E_GFX=1.
                    if matches!(dirty, DirtyState::Clean) {
                        e2e_probe.maybe_synth_cursor_cycle(&surfaces, &mut dirty);
                    }
                }
                Err(_) => return 0xfc0c,
        }
    }
}
