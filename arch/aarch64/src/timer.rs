pub const EL1_PHYSICAL_TIMER_ENABLE: u64 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimerArmError {
    CounterFrequencyUnavailable,
}

pub const fn cycles_per_tick(counter_frequency_hz: u64, tick_hz: u64) -> u64 {
    let frequency = if counter_frequency_hz == 0 {
        1
    } else {
        counter_frequency_hz
    };
    let rate = if tick_hz == 0 { 1 } else { tick_hz };
    let interval = frequency / rate;
    if interval == 0 { 1 } else { interval }
}

pub const fn next_compare_value(now: u64, previous_compare: u64, interval_cycles: u64) -> u64 {
    let interval = if interval_cycles == 0 {
        1
    } else {
        interval_cycles
    };
    if previous_compare == 0 {
        return now.saturating_add(interval);
    }
    let candidate = previous_compare.saturating_add(interval);
    if candidate <= now {
        now.saturating_add(interval)
    } else {
        candidate
    }
}

#[cfg(target_arch = "aarch64")]
mod imp {
    use core::arch::asm;
    use core::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static TICK_INTERVAL_CYCLES: AtomicU64 = AtomicU64::new(0);
    static NEXT_COMPARE_VALUE: AtomicU64 = AtomicU64::new(0);

    fn counter_frequency_hz() -> u64 {
        let value: u64;
        unsafe {
            asm!(
                "mrs {value}, cntfrq_el0",
                value = out(reg) value,
                options(nomem, nostack, preserves_flags)
            );
        }
        value
    }

    fn counter_value() -> u64 {
        let value: u64;
        unsafe {
            asm!(
                "mrs {value}, cntpct_el0",
                value = out(reg) value,
                options(nostack)
            );
        }
        value
    }

    fn write_compare(value: u64) {
        unsafe {
            asm!(
                "msr cntp_cval_el0, {value}",
                value = in(reg) value,
                options(nostack)
            );
        }
    }

    fn write_control(enable: bool) {
        let value = if enable { EL1_PHYSICAL_TIMER_ENABLE } else { 0 };
        unsafe {
            asm!(
                "msr cntp_ctl_el0, {value}",
                value = in(reg) value,
                options(nostack)
            );
        }
    }

    pub fn arm_periodic_tick(tick_hz: u64) -> Result<u64, TimerArmError> {
        let frequency = counter_frequency_hz();
        if frequency == 0 {
            return Err(TimerArmError::CounterFrequencyUnavailable);
        }
        let interval = cycles_per_tick(frequency, tick_hz);

        write_control(false);
        let now = counter_value();
        let compare = now.saturating_add(interval);
        NEXT_COMPARE_VALUE.store(compare, Ordering::Relaxed);
        TICK_INTERVAL_CYCLES.store(interval, Ordering::Relaxed);
        write_compare(compare);
        write_control(true);
        Ok(interval)
    }

    pub fn rearm_periodic_tick() -> u64 {
        let interval = TICK_INTERVAL_CYCLES.load(Ordering::Relaxed);
        if interval == 0 {
            return 0;
        }
        let now = counter_value();
        let previous = NEXT_COMPARE_VALUE.load(Ordering::Relaxed);
        let next = next_compare_value(now, previous, interval);
        NEXT_COMPARE_VALUE.store(next, Ordering::Relaxed);
        write_compare(next);
        next
    }

    pub fn tick_interval_cycles() -> u64 {
        TICK_INTERVAL_CYCLES.load(Ordering::Relaxed)
    }

    pub fn disarm() {
        write_control(false);
        TICK_INTERVAL_CYCLES.store(0, Ordering::Relaxed);
        NEXT_COMPARE_VALUE.store(0, Ordering::Relaxed);
    }
}

#[cfg(not(target_arch = "aarch64"))]
mod imp {
    use super::*;

    pub fn arm_periodic_tick(_tick_hz: u64) -> Result<u64, TimerArmError> {
        Err(TimerArmError::CounterFrequencyUnavailable)
    }

    pub fn rearm_periodic_tick() -> u64 {
        0
    }

    pub fn tick_interval_cycles() -> u64 {
        0
    }

    pub fn disarm() {}
}

pub use imp::{arm_periodic_tick, disarm, rearm_periodic_tick, tick_interval_cycles};
