//! E2E input witness counters (docs/test-plan.md §3 item 4). The desktop
//! shell is the terminal sink of the kernel->session-service input route;
//! counting deliveries here proves events flow end to end without any
//! visual assertion. Fully inert unless built with SERVICEOS_E2E_INPUT=1.
//!
//! Line protocol (event-milestone cadence; no monotonic-clock dependency):
//!   E2E input.counters delivered=<n> lost=<m>
//! `delivered` counts dispatches that reached handle_input; `lost` counts
//! pointer moves the desktop coalesced away (fresher sample replaced a
//! still-pending one) — the shell-visible portion of plan's delivered/lost
//! split, since scheduler wakeup-latch internals are kernel-side.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::rt;

/// Builds with SERVICEOS_E2E_INPUT=1 expose the counters.
pub(crate) fn enabled() -> bool {
    matches!(option_env!("SERVICEOS_E2E_INPUT"), Some("1"))
}

static DELIVERED: AtomicU64 = AtomicU64::new(0);
static LOST: AtomicU64 = AtomicU64::new(0);
/// Next emission milestone (first event, then every EMIT_EVERY more).
static NEXT_EMIT: AtomicU64 = AtomicU64::new(1);

const EMIT_EVERY: u64 = 4;

pub(crate) fn note_delivered() {
    if !enabled() {
        return;
    }
    let delivered = DELIVERED.fetch_add(1, Ordering::Relaxed) + 1;
    maybe_emit(delivered, LOST.load(Ordering::Relaxed));
}

pub(crate) fn note_coalesced_drop() {
    if !enabled() {
        return;
    }
    LOST.fetch_add(1, Ordering::Relaxed);
}

fn maybe_emit(delivered: u64, lost: u64) {
    loop {
        let next = NEXT_EMIT.load(Ordering::Relaxed);
        if delivered < next {
            return;
        }
        // Advance in fixed strides so milestone math stays honest even when
        // several events land between wakeups.
        if NEXT_EMIT
            .compare_exchange(
                next,
                delivered + EMIT_EVERY,
                Ordering::Relaxed,
                Ordering::Relaxed,
            )
            .is_ok()
        {
            let _ = rt::write_logf(
                "desktop-shell",
                format_args!("E2E input.counters delivered={delivered} lost={lost}"),
            );
            return;
        }
    }
}
