//! E2E witness probes for the compositor (docs/test-plan.md §3 item 5 +
//! §T4 regress.cursor-band-flush). Everything here is inert unless the guest
//! image was built with SERVICEOS_E2E_GFX=1; default boots emit nothing and
//! take no extra branches that could drift serial bytes.
//!
//! Line protocol:
//!   E2E gfx.present outputs=<k> frames=<n> fences=<n>
//!     emitted on the first present and every PRESENT_EMIT_EVERY-th frame;
//!     two matching lines prove the present counter advances.
//!   E2E gfx.cursor-band cycle=<k> visible=1
//!   E2E gfx.cursor-band FAIL cycle=<k> found=<0|1> visible=<0|1>
//!     one per synthetic cursor-band cycle. Each cycle replays the exact
//!     geometry that pinned dd7d1f3 ("keep cursor layer visible under
//!     present optimization") through the real plan_band_flush planner:
//!     a damage rect whose row prefix is identical to the presented shadow
//!     must NOT be classified Skip just because a column-0 anchored compare
//!     reads an unchanged prefix outside the damage columns.

use serviceos_userspace_runtime as rt;

use crate::compose::{BandAction, plan_band_flush};
use crate::types::{
    DamageRect, DamageSet, DirtyState, Surfaces, is_cursor_surface, surface_bounds,
};

/// Builds with SERVICEOS_E2E_GFX=1 run the witness probes.
pub(crate) fn enabled() -> bool {
    matches!(option_env!("SERVICEOS_E2E_GFX"), Some("1"))
}

/// Idle loop wakeups between synthetic cursor-band cycles.
const CYCLE_EVERY_IDLE_WAKEUPS: u32 = 16;
/// Upper bound on synth cycles so long boots cannot spam serial endlessly.
const MAX_SYNTH_CYCLES: u32 = 64;
/// Emit an `E2E gfx.present` line at this frame-count cadence (plus frame 1).
const PRESENT_EMIT_EVERY: u64 = 16;

pub(crate) struct GfxProbe {
    cycles: u32,
    idle_wakeups: u32,
    next_emit_at: u64,
}

impl GfxProbe {
    pub(crate) const fn new() -> Self {
        Self {
            cycles: 0,
            idle_wakeups: 0,
            next_emit_at: 1,
        }
    }

    /// Called after every successful present. Advances emission milestones
    /// so host witnesses can prove the present counter advances.
    pub(crate) fn note_present(&mut self, frames_now: u64, fences_completed: u64) {
        if !enabled() || frames_now == 0 {
            return;
        }
        if frames_now >= self.next_emit_at {
            self.next_emit_at = frames_now + PRESENT_EMIT_EVERY;
            let _ = rt::write_logf(
                "graphics",
                format_args!(
                    "E2E gfx.present outputs=1 frames={frames_now} fences={fences_completed}"
                ),
            );
        }
    }

    /// Called each idle QueueEmpty wakeup while Clean. On its interval it
    /// (a) replays one dd7d1f3 geometry cycle through plan_band_flush and
    /// (b) replants cursor-area damage so idle boots keep issuing REAL band
    /// presents, advancing `present_count` for the host witness. Returns
    /// true when this wakeup drove either mechanism.
    pub(crate) fn maybe_synth_cursor_cycle(
        &mut self,
        surfaces: &Surfaces,
        dirty: &mut DirtyState,
    ) -> bool {
        if !enabled() || self.cycles >= MAX_SYNTH_CYCLES {
            return false;
        }
        self.idle_wakeups += 1;
        if self.idle_wakeups < CYCLE_EVERY_IDLE_WAKEUPS {
            return false;
        }
        self.idle_wakeups = 0;
        let planted = self.plant_cursor_band(surfaces, dirty);
        let _assert_ran = self.run_cursor_band_cycle();
        planted || _assert_ran
    }

    /// Plants a synthetic cursor-surface damage band so the next poll issues
    /// a genuine partial present through compose/present (advancing the
    /// real present counter).
    fn plant_cursor_band(&mut self, surfaces: &Surfaces, dirty: &mut DirtyState) -> bool {
        if !matches!(dirty, DirtyState::Clean) {
            return false;
        }
        let Some(band) = cursor_band(surfaces) else {
            // No cursor surface alive yet: nothing to plant (factory boots
            // create one, but TCG timing varies).
            return false;
        };
        let damages = DamageSet::empty().push(band);
        *dirty = DirtyState::Region {
            damages,
            immediate: true,
        };
        true
    }

    /// dd7d1f3 regression replay: 64x4 output, damage at columns 48..56 of
    /// rows 1..3 whose row PREFIX (columns 0..48) matches the presented
    /// shadow exactly. A column-clipped compare must see the changed columns
    /// and demand band flushes; anchoring at column 0 would read the matching
    /// prefix instead and classify the region as an unchanged Skip.
    fn run_cursor_band_cycle(&mut self) -> bool {
        let width = 64u32;
        let height = 4u32;
        let bpp = 4u32;
        let output = rt::DisplayOutputInfo {
            backend: 0,
            state: 0,
            pixel_format: 0,
            reserved: 0,
            width,
            height,
            stride: width,
            bytes_per_pixel: bpp,
            byte_len: u64::from(width) * u64::from(height) * u64::from(bpp),
            present_count: 0,
        };
        // no_std build: fixed-size buffers, no alloc-macro dependency.
        let mut frame = [0u8; 1024];
        let presented = [0u8; 1024];
        // Damage columns x=48..56 across rows 1..3 change; the prefix columns
        // 0..48 of those same rows stay identical to the shadow.
        for row in 1..3usize {
            for x in 48..56usize {
                let offset = (row * width as usize + x) * bpp as usize;
                frame[offset] = 0xff;
            }
        }
        let clip = Some(DamageRect {
            x: 48,
            y: 1,
            width: 8,
            height: 2,
        });
        let plan = plan_band_flush(&frame, &presented, output, clip, true);
        self.cycles = self.cycles.saturating_add(1);
        match plan.action {
            BandAction::Skip(saved_bytes) => {
                let _ = rt::write_logf(
                    "graphics",
                    format_args!(
                        "E2E gfx.cursor-band FAIL cycle={} found=1 visible=0 skip_bytes={}",
                        self.cycles, saved_bytes
                    ),
                );
            }
            BandAction::Bands { saved_bytes } => {
                let _ = rt::write_logf(
                    "graphics",
                    format_args!(
                        "E2E gfx.cursor-band cycle={} visible=1 saved={saved_bytes}",
                        self.cycles
                    ),
                );
            }
            BandAction::WholeClip => {
                let _ = rt::write_logf(
                    "graphics",
                    format_args!(
                        "E2E gfx.cursor-band cycle={} visible=1 whole_clip=1",
                        self.cycles
                    ),
                );
            }
        }
        true
    }

    /// Cycle counter readback for tests / future callers.
    #[allow(dead_code)]
    pub(crate) fn cycles(&self) -> u32 {
        self.cycles
    }
}

/// Cursor-damage band with a few pixels of padding on each side so the flush
/// covers both cursor and neighboring background scanlines.
fn cursor_band(surfaces: &Surfaces) -> Option<DamageRect> {
    for surface in surfaces.iter() {
        if !surface.occupied || !is_cursor_surface(surface) {
            continue;
        }
        let bounds = surface_bounds(surface);
        if bounds.width == 0 || bounds.height == 0 {
            return None;
        }
        let pad = 2i32;
        let x = (bounds.x - pad).max(0);
        let y = (bounds.y - pad).max(0);
        let width = bounds.width + 2 * pad as u32;
        let height = bounds.height + 2 * pad as u32;
        return Some(DamageRect {
            x,
            y,
            width,
            height,
        });
    }
    None
}
