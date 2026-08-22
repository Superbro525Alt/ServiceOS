//! Host-side unit tests for the HPET driver's pure math.
//!
//! Compiled standalone with the system toolchain so the no_std kernel
//! allocator is never linked:
//!
//! ```sh
//! rustc --test tests/hpet_math_host.rs -o /tmp/hpet_math_host \
//!     && /tmp/hpet_math_host
//! ```

// Pull in the driver's dependency-free math module by textual inclusion so
// exactly the code that ships in the kernel gets exercised here.
#[path = "../src/hpet_math.rs"]
mod hpet_math;

use hpet_math::{
    femtoseconds_to_nanoseconds, frequency_hz, periodic_fit_ticks, PeriodicFitError,
    MAX_CLOCK_PERIOD_FS,
};

#[test]
fn femtoseconds_to_nanoseconds_converts_by_truncation() {
    assert_eq!(femtoseconds_to_nanoseconds(1_000_000), Some(1));
    assert_eq!(femtoseconds_to_nanoseconds(10_000_000), Some(10));
    assert_eq!(femtoseconds_to_nanoseconds(1_500_000), Some(1));
    assert_eq!(femtoseconds_to_nanoseconds(999_999), Some(0));
}

#[test]
fn frequency_hz_inverts_clock_period() {
    // QEMU's HPET: 10 ns per tick == 100 MHz
    assert_eq!(frequency_hz(10_000_000), 100_000_000);
    // 1 GHz clock
    assert_eq!(frequency_hz(1_000_000), 1_000_000_000);
    // Spec maximum 100 ns period == 10 MHz clock
    assert_eq!(frequency_hz(MAX_CLOCK_PERIOD_FS as u32), 10_000_000);
    assert_eq!(frequency_hz(0), 0);
}

#[test]
fn periodic_fit_accepts_clean_divisions_within_register_width() {
    // 1 ms (1e12 fs) on a 10 ns-tick counter = exactly 100_000 ticks
    assert_eq!(periodic_fit_ticks(1_000_000_000_000, 10_000_000), Ok(100_000));
    // 1 us (1e9 fs) on the same counter = exactly 100 ticks
    assert_eq!(periodic_fit_ticks(1_000_000_000, 10_000_000), Ok(100));
    // 1 ms on a 1 fs-tick counter = 10^12 ticks: exceeds u32::MAX
    assert_eq!(
        periodic_fit_ticks(1_000_000_000_000, 1),
        Err(PeriodicFitError::DeltaTooLarge)
    );
    assert_eq!(
        periodic_fit_ticks(u64::MAX, 1),
        Err(PeriodicFitError::DeltaTooLarge)
    );
}

#[test]
fn periodic_fit_rejects_misaligned_intervals() {
    // One femtosecond of remainder against a 10 ns tick: inexact.
    assert_eq!(
        periodic_fit_ticks(1_000_000_001, 10_000_000),
        Err(PeriodicFitError::InexactDivision)
    );
}

#[test]
fn periodic_fit_rejects_sub_tick_and_degenerate_periods() {
    assert_eq!(
        periodic_fit_ticks(5, 10_000_000),
        Err(PeriodicFitError::DeltaTooSmall)
    );
    assert_eq!(
        periodic_fit_ticks(1_000_000, 0),
        Err(PeriodicFitError::InexactDivision)
    );
}
