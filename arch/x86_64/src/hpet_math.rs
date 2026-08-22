//! Standalone pure-math helpers for the HPET driver.
//!
//! Deliberately dependency-free (no kernel-core, no alloc) so this file can
//! be compiled and unit-tested on the host in isolation:
//! `rustc --test arch/x86_64/tests/hpet_math_host.rs && ./hpet_math_host`
//! The test harness includes this source directly.

/// Convert femtoseconds to whole nanoseconds by truncation.
pub fn femtoseconds_to_nanoseconds(femtoseconds: u64) -> Option<u64> {
    const FS_PER_NS: u64 = 1_000_000;
    femtoseconds.checked_div(FS_PER_NS)
}

/// Counter frequency in hertz implied by a tick period given in femtoseconds.
///
/// A zero period yields zero rather than a division panic; callers treat
/// zero as invalid upstream.
pub fn frequency_hz(clock_period_fs: u32) -> u64 {
    const FS_PER_SECOND: u64 = 1_000_000_000_000_000;
    if clock_period_fs == 0 {
        return 0;
    }
    FS_PER_SECOND / clock_period_fs as u64
}

/// Spec-mandated maximum clock period: 0x05F5E100 fs == 100 ns (10 MHz)
pub const MAX_CLOCK_PERIOD_FS: u64 = 0x05F5_E100;

/// Why a periodic timer-0 fit was rejected
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeriodicFitError {
    /// Comparator does not advertise periodic mode support.
    ///
    /// Constructed by the hardware driver (`hpet.rs`), which reads the
    /// capability bit; the pure math never produces it, hence the allow.
    #[allow(dead_code)]
    NotPeriodicCapable,
    /// Interval is not an exact multiple of the counter period
    InexactDivision,
    /// Required comparator delta does not fit the 32-bit comparator register
    DeltaTooLarge,
    /// Required comparator delta rounds to zero ticks
    DeltaTooSmall,
}

/// Comparator delta implementing `interval_fs` on a counter ticking every
/// `clock_period_fs`, requiring an exact ("clean") fit into the 32-bit
/// comparator register.
pub fn periodic_fit_ticks(
    interval_fs: u64,
    clock_period_fs: u32,
) -> Result<u32, PeriodicFitError> {
    if clock_period_fs == 0 {
        return Err(PeriodicFitError::InexactDivision);
    }
    let period = clock_period_fs as u64;
    let delta = interval_fs / period;
    if interval_fs % period != 0 || delta == 0 {
        // Distinguish "too coarse" (rounds to nothing) from "misaligned".
        return if delta > 0 {
            Err(PeriodicFitError::InexactDivision)
        } else {
            Err(PeriodicFitError::DeltaTooSmall)
        };
    }
    if delta > u32::MAX as u64 {
        return Err(PeriodicFitError::DeltaTooLarge);
    }
    Ok(delta as u32)
}
