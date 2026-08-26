//! Timer access through the SBI TIME extension plus the `time` CSR.
//!
//! Skeleton scope: read the running counter and schedule one-shot compares
//! via SBI `set_timer`. Periodic-tick integration into a scheduler is open.


/// QEMU `virt` ships a 10 MHz `timebase-frequency`; real boards must parse
/// this from the device tree once DTB support lands.
pub const QEMU_VIRT_TIMEBASE_HZ: u64 = 10_000_000;

pub const TIMER_TICK_HZ: u64 = 100;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimerArmError {
    ZeroInterval,
}

pub const fn cycles_per_tick(timebase_hz: u64, tick_hz: u64) -> u64 {
    let frequency = if timebase_hz == 0 { 1 } else { timebase_hz };
    let rate = if tick_hz == 0 { 1 } else { tick_hz };
    let interval = frequency / rate;
    if interval == 0 { 1 } else { interval }
}

#[cfg(target_arch = "riscv64")]
pub fn now() -> u64 {
    let value: u64;
    unsafe {
        core::arch::asm!("rdtime {value}", value = out(reg) value, options(nomem, nostack));
    }
    value
}

#[cfg(target_arch = "riscv64")]
pub fn arm_oneshot_tick(tick_hz: u64) -> Result<u64, TimerArmError> {
    if tick_hz == 0 || tick_hz > QEMU_VIRT_TIMEBASE_HZ {
        return Err(TimerArmError::ZeroInterval);
    }
    let interval = cycles_per_tick(QEMU_VIRT_TIMEBASE_HZ, tick_hz) as u64;
    sbi::set_timer(now().saturating_add(interval));
    Ok(interval)
}

#[cfg(not(target_arch = "riscv64"))]
pub fn now() -> u64 {
    0
}

#[cfg(not(target_arch = "riscv64"))]
pub fn arm_oneshot_tick(_tick_hz: u64) -> Result<u64, TimerArmError> {
    Err(TimerArmError::ZeroInterval)
}
