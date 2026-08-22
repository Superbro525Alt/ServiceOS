#[cfg(target_arch = "aarch64")]
mod imp {
    use core::arch::asm;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct TimerStatus {
        pub implemented: bool,
    }

    pub const fn status() -> TimerStatus {
        TimerStatus { implemented: true }
    }

    pub fn counter_frequency_hz() -> u64 {
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

    pub fn counter_value() -> u64 {
        let value: u64;
        unsafe {
            asm!(
                "mrs {value}, cntpct_el0",
                value = out(reg) value,
                options(nomem, nostack, preserves_flags)
            );
        }
        value
    }
}

#[cfg(not(target_arch = "aarch64"))]
mod imp {
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct TimerStatus {
        pub implemented: bool,
    }

    pub const fn status() -> TimerStatus {
        TimerStatus { implemented: false }
    }

    pub fn counter_frequency_hz() -> u64 {
        0
    }

    pub fn counter_value() -> u64 {
        0
    }
}

pub use imp::{TimerStatus, counter_frequency_hz, counter_value, status};
