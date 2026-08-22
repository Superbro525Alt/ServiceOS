//! High Precision Event Timer driver behind ACPI runtime detection.
//!
//! Discovery walks RSDP → XSDT/RSDT for the `HPET` Description Table and
//! maps the event-timer block MMIO through the kernel's offset-0 direct map
//! (the same convention as the LAPIC driver). This pass validates presence,
//! capability sanity, and main-counter liveness, then programs timer 0 in
//! periodic mode **with its interrupt output left disabled** so the LAPIC
//! timer remains the sole system tick source. The public surface is shaped
//! for future wake scheduling only.

use core::ptr;
use spin::Mutex;

use crate::{acpi, serial};
use serviceos_kernel_core::memory::PhysicalAddress;

pub use crate::hpet_math::{
    femtoseconds_to_nanoseconds, frequency_hz, periodic_fit_ticks, PeriodicFitError,
};

/// Spec-mandated maximum clock period: 0x05F5E100 fs == 100 ns (10 MHz)
const MAX_CLOCK_PERIOD_FS: u64 = crate::hpet_math::MAX_CLOCK_PERIOD_FS;

/// General capabilities and ID register offset
const GCAP_ID: usize = 0x000;
/// General configuration register offset
const GEN_CONF: usize = 0x010;
/// Main counter register offset
const MAIN_COUNTER: usize = 0x0F0;
/// Timer 0 configuration register offset
const TIMER0_CONFIG: usize = 0x100;
/// Offset of the comparator (interval) register within a timer block
const TIMER_COMPARATOR_OFFSET: usize = 0x08;

/// General configuration: overall enable
const CONF_ENABLE: u64 = 1 << 0;
/// Timer config: interrupt enable (deliberately never set in this pass)
const TIMER_INT_ENABLE: u64 = 1 << 2;
/// Timer config: periodic mode (same bit across spec and QEMU)
const TIMER_TYPE_PERIODIC: u64 = 1 << 3;
/// Timer config: periodic-capable advertisement.
///
/// The HPET v1.0a spec puts TN_PER_CAP at bit 7; QEMU's device model
/// advertises it at bit 4 instead (its SIZE_CAP sits at bit 5). Accept
/// either so the driver works on both real silicon and QEMU.
const TIMER_PERIODIC_CAP: u64 = (1 << 7) | (1 << 4);
/// Timer config: comparator write loads the periodic interval directly.
///
/// Spec places TN_VAL_SET_CNF at bit 5, QEMU accepts its SETVAL at bit 6.
/// Both are written: each implementation honors its own bit and ignores
/// the other (QEMU's write mask drops bit 5, real silicon treats bit 6
/// as the read-only SIZE_CAP).
const TIMER_VALUE_SET: u64 = (1 << 5) | (1 << 6);

/// GCAP fields: revision id [31:16], timer count minus one [12:8],
/// 64-bit counter capable [13], clock period in femtoseconds [63:32]
const CAP_REV_ID_MASK: u64 = 0xFFFF << 16;
const CAP_TIMER_COUNT_SHIFT: u64 = 8;
const CAP_TIMER_COUNT_MASK: u64 = 0x1F << CAP_TIMER_COUNT_SHIFT;
const CAP_COUNTER_64BIT: u64 = 1 << 13;
const CAP_CLOCK_PERIOD_SHIFT: u64 = 32;

/// Sanity floor: anything below 1 fs cannot come out of real silicon
const MIN_CLOCK_PERIOD_FS: u64 = 1;
/// Bounded spins while polling the main counter for signs of life
const COUNTER_POLL_SPINS: usize = 200_000;
/// Target interval probed for a clean periodic fit: 1 ms expressed in fs
const TARGET_INTERVAL_FS: u64 = 1_000_000_000_000;

/// Validated HPET block description
#[derive(Clone, Copy, Debug)]
pub struct HpetInfo {
    /// Physical MMIO base of the event timer block (identity-mapped)
    pub base_address: u64,
    /// Number of comparators in the block
    pub num_timers: u32,
    /// Main-counter tick period in femtoseconds
    pub clock_period_fs: u32,
    /// Whether the main counter is 64-bit wide
    pub counter_is_64bit: bool,
}

impl HpetInfo {
    /// Main-counter frequency in Hz
    pub fn frequency_hz(&self) -> u64 {
        frequency_hz(self.clock_period_fs)
    }
}

static HPET: Mutex<Option<HpetInfo>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// MMIO access (offset-0 identity/direct map, matching lapic.rs)
// ---------------------------------------------------------------------------

unsafe fn read_register(base_address: u64, offset: usize) -> u64 {
    unsafe { ptr::read_volatile((base_address + offset as u64) as *const u64) }
}

unsafe fn write_register(base_address: u64, offset: usize, value: u64) {
    unsafe { ptr::write_volatile((base_address + offset as u64) as *mut u64, value) }
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Probe for an ACPI-described HPET, validate it, and program timer 0 for a
/// future wake-scheduling role without touching the system tick source.
///
/// Emits exactly one serial line either way: `hpet: present base=... timers=N
/// freq=...Hz` (optionally suffixed with what failed validation) or
/// `hpet: absent`.
pub fn initialize(rsdp_address: Option<PhysicalAddress>) {
    let mut guard = HPET.lock();
    if guard.is_some() {
        return;
    }

    let Some(base_address) = acpi::hpet_base_address(rsdp_address) else {
        serial::write_args(format_args!("serviceos: hpet: absent\n"));
        return;
    };

    // Safety: the ACPI table handed us this physical MMIO base and the
    // kernel's direct map makes it readable; no other code touches it yet.
    unsafe {
        // Bring the whole block up before reading capabilities; QEMU gates
        // the counter on this bit.
        let general_config = read_register(base_address, GEN_CONF);
        write_register(base_address, GEN_CONF, general_config | CONF_ENABLE);

        let caps = read_register(base_address, GCAP_ID);
        let clock_period_fs = (caps >> CAP_CLOCK_PERIOD_SHIFT) as u32;
        let num_timers = (((caps & CAP_TIMER_COUNT_MASK) >> CAP_TIMER_COUNT_SHIFT) + 1) as u32;
        let counter_is_64bit = caps & CAP_COUNTER_64BIT != 0;
        let revision = ((caps & CAP_REV_ID_MASK) >> 16) as u16;

        let info = HpetInfo {
            base_address,
            num_timers,
            clock_period_fs,
            counter_is_64bit,
        };

        // Capability sanity: spec-bounded clock period, at least one timer,
        // nonzero revision.
        if revision == 0
            || num_timers == 0
            || (clock_period_fs as u64) < MIN_CLOCK_PERIOD_FS
            || (clock_period_fs as u64) > MAX_CLOCK_PERIOD_FS
        {
            serial::write_args(format_args!(
                "serviceos: hpet: present base={:#x} timers={} freq={}Hz invalid-caps\n",
                base_address,
                num_timers,
                info.frequency_hz(),
            ));
            return;
        }

        // Liveness: the main counter must advance within a bounded poll.
        let start = read_register(base_address, MAIN_COUNTER);
        let mut advanced = false;
        for _ in 0..COUNTER_POLL_SPINS {
            if read_register(base_address, MAIN_COUNTER) != start {
                advanced = true;
                break;
            }
            core::hint::spin_loop();
        }
        if !advanced {
            serial::write_args(format_args!(
                "serviceos: hpet: present base={:#x} timers={} freq={}Hz counter-stuck\n",
                base_address,
                num_timers,
                info.frequency_hz(),
            ));
            return;
        }

        // Program timer 0 for a clean periodic fit with the interrupt
        // output masked: counting proceeds, no IRQ can ever fire, and the
        // LAPIC stays the system tick source.
        let observed_config = read_register(base_address, TIMER0_CONFIG);
        match program_timer0_periodic(base_address, observed_config, TARGET_INTERVAL_FS, clock_period_fs) {
            Ok(delta) => {
                *guard = Some(info);
                serial::write_args(format_args!(
                    "serviceos: hpet: present base={:#x} timers={} freq={}Hz timer0-periodic-delta={}\n",
                    base_address,
                    num_timers,
                    info.frequency_hz(),
                    delta,
                ));
            }
            Err(error) => {
                serial::write_args(format_args!(
                    "serviceos: hpet: present base={:#x} timers={} freq={}Hz timer0-fit={:?} timer0-cfg={:#x}\n",
                    base_address,
                    num_timers,
                    info.frequency_hz(),
                    error,
                    observed_config,
                ));
            }
        }
    }
}

/// Snapshot of the validated HPET block, if discovery succeeded
pub fn info() -> Option<HpetInfo> {
    *HPET.lock()
}

/// Program timer 0 in periodic mode for `interval_fs` per reload.
///
/// Uses the VAL_SET sequence so the comparator write loads the reload
/// interval rather than an absolute compare point. The interrupt enable
/// bit stays clear throughout: the device counts but never asserts its
/// output, so no IDT vector or I/O APIC routing is required.
fn program_timer0_periodic(
    base_address: u64,
    observed_timer0_config: u64,
    interval_fs: u64,
    clock_period_fs: u32,
) -> Result<u32, PeriodicFitError> {
    // Periodic-capable advertisement differs between the HPET spec (bit 7)
    // and QEMU's model (bit 4); see TIMER_PERIODIC_CAP.
    if observed_timer0_config & TIMER_PERIODIC_CAP == 0 {
        return Err(PeriodicFitError::NotPeriodicCapable);
    }
    let delta = periodic_fit_ticks(interval_fs, clock_period_fs)?;

    // The interrupt-enable bit must never be set: this driver counts but
    // never raises IRQs, so no IDT vector or I/O APIC routing is required.
    let masked_config = TIMER_TYPE_PERIODIC & !TIMER_INT_ENABLE;

    unsafe {
        // Select periodic mode with direct comparator-load semantics for
        // the next write (VAL_SET), interrupt output already masked.
        write_register(base_address, TIMER0_CONFIG, masked_config | TIMER_VALUE_SET);
        write_register(
            base_address,
            TIMER0_CONFIG + TIMER_COMPARATOR_OFFSET,
            delta as u64,
        );
        // Clear VAL_SET: subsequent behavior is plain periodic reload.
        write_register(base_address, TIMER0_CONFIG, masked_config);
        debug_assert!(
            read_register(base_address, TIMER0_CONFIG) & TIMER_INT_ENABLE == 0,
            "hpet timer0 interrupt enable must stay clear"
        );
    }
    Ok(delta)
}

/// Compute the comparator delta a future wake scheduler would program to
/// expire after `delay_ns`, against the discovered block.
///
/// Pure lookup over validated state; returns `None` when no HPET is present
/// or the delay has no clean fit. Does not touch hardware.
pub fn wake_ticks_for_delay(delay_ns: u64) -> Option<u32> {
    let info = info()?;
    let delay_fs = delay_ns.checked_mul(1_000_000)?;
    periodic_fit_ticks(delay_fs, info.clock_period_fs).ok()
}
