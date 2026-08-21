use core::arch::asm;
use spin::Mutex;

/// Local APIC registers (memory-mapped)
const LAPIC_BASE: u64 = 0x0000_0000_FEE0_0000;

/// LAPIC register offsets
const LAPIC_ID: u32 = 0x020;
const LAPIC_TPR: u32 = 0x080;
const LAPIC_EOI: u32 = 0x0B0;
const LAPIC_LVT_TIMER: u32 = 0x320;
const LAPIC_TIMER_ICR: u32 = 0x380;
const LAPIC_TIMER_DCR: u32 = 0x3E0;

/// LAPIC timer modes
const LVT_TIMER_MODE_PERIODIC: u32 = 1 << 17;
const LVT_TIMER_MASKED: u32 = 1 << 16;

/// LAPIC timer divide configurations
const TDCR_DIV_BY_1: u32 = 0x0B;
const TDCR_DIV_BY_16: u32 = 0x03;
const TDCR_DIV_BY_256: u32 = 0x09;

/// LAPIC timer driver
pub struct LapicTimer {
    base_address: u64,
    ticks_per_ms: u32,
    initialized: bool,
}

impl LapicTimer {
    /// Create a new LAPIC timer driver
    pub const fn new() -> Self {
        Self {
            base_address: LAPIC_BASE,
            ticks_per_ms: 0,
            initialized: false,
        }
    }

    /// Read a LAPIC register
    unsafe fn read_reg(&self, offset: u32) -> u32 {
        let addr = self.base_address + offset as u64;
        let value: u32;
        unsafe {
            asm!(
                "mov {0:e}, [{1}]",
                out(reg) value,
                in(reg) addr,
                options(nostack),
            );
        }
        value
    }

    /// Write a LAPIC register
    unsafe fn write_reg(&self, offset: u32, value: u32) {
        let addr = self.base_address + offset as u64;
        unsafe {
            asm!(
                "mov [{0}], {1:e}",
                in(reg) addr,
                in(reg) value,
                options(nostack),
            );
        }
    }

    /// Initialize the LAPIC timer
    ///
    /// # Safety
    /// This function must be called with interrupts disabled and the
    /// LAPIC memory region must be mapped and accessible.
    pub unsafe fn initialize(&mut self, frequency_hint: u32) {
        if self.initialized {
            return;
        }

        // Disable interrupts in LAPIC
        unsafe {
            self.write_reg(LAPIC_TPR, 0x0000_00FF);
        }

        // Set timer to periodic mode with divide by 16
        unsafe {
            self.write_reg(LAPIC_LVT_TIMER, LVT_TIMER_MODE_PERIODIC | 0x20);
            self.write_reg(LAPIC_TIMER_DCR, TDCR_DIV_BY_16);
        }

        // Calibrate the timer by measuring against the PIT
        // For now, use a simple calibration based on the frequency hint
        self.ticks_per_ms = frequency_hint / 16 / 1000;

        // Set initial count
        unsafe {
            self.write_reg(LAPIC_TIMER_ICR, self.ticks_per_ms * 10);
        }

        self.initialized = true;
    }

    /// Start the LAPIC timer
    pub unsafe fn start(&self, ticks: u32) {
        if !self.initialized {
            return;
        }
        unsafe {
            self.write_reg(LAPIC_TIMER_ICR, ticks);
        }
    }

    /// Stop the LAPIC timer
    pub unsafe fn stop(&self) {
        if !self.initialized {
            return;
        }
        unsafe {
            self.write_reg(LAPIC_TIMER_ICR, 0);
        }
    }

    /// Send EOI (End of Interrupt) to the LAPIC
    pub unsafe fn send_eoi(&self) {
        if !self.initialized {
            return;
        }
        unsafe {
            self.write_reg(LAPIC_EOI, 0);
        }
    }

    /// Check if the LAPIC timer is initialized
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Get the ticks per millisecond
    pub fn ticks_per_ms(&self) -> u32 {
        self.ticks_per_ms
    }
}

/// Global LAPIC timer instance
static LAPIC_TIMER: Mutex<LapicTimer> = Mutex::new(LapicTimer::new());

/// Initialize the LAPIC timer
///
/// # Safety
/// This function must be called with interrupts disabled and the
/// LAPIC memory region must be mapped and accessible.
pub unsafe fn initialize(frequency_hint: u32) {
    unsafe {
        LAPIC_TIMER.lock().initialize(frequency_hint);
    }
}

/// Get a reference to the global LAPIC timer
pub fn timer() -> spin::MutexGuard<'static, LapicTimer> {
    LAPIC_TIMER.lock()
}

/// Send EOI to the LAPIC
///
/// # Safety
/// This function must be called from an interrupt handler.
pub unsafe fn send_eoi() {
    unsafe {
        LAPIC_TIMER.lock().send_eoi();
    }
}
