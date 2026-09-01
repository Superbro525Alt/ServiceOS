use core::arch::asm;
use spin::Mutex;

/// Default local APIC MMIO base; overridden by IA32_APIC_BASE if firmware relocated the APIC.
const DEFAULT_LAPIC_BASE: u64 = 0x0000_0000_FEE0_0000;

/// IA32_APIC_BASE MSR and its fields
const MSR_IA32_APIC_BASE: u32 = 0x1B;
const APIC_BASE_ADDR_MASK: u64 = 0xFFFF_FFFF_FFFF_F000;
const APIC_BASE_GLOBAL_ENABLE: u64 = 1 << 11;

/// LAPIC register offsets
const LAPIC_ID: u32 = 0x020;
const LAPIC_TPR: u32 = 0x080;
const LAPIC_EOI: u32 = 0x0B0;
const LAPIC_SPURIOUS: u32 = 0x0F0;
const LAPIC_ICR_LOW: u32 = 0x300;
const LAPIC_ICR_HIGH: u32 = 0x310;
const LAPIC_LVT_TIMER: u32 = 0x320;
const LAPIC_LVT_LINT0: u32 = 0x350;
const LAPIC_TIMER_ICR: u32 = 0x380;
const LAPIC_TIMER_CCR: u32 = 0x390;
const LAPIC_TIMER_DCR: u32 = 0x3E0;

/// LAPIC LVT entry bits
const LVT_MODE_EXTINT: u32 = 0b111 << 8;
/// Timer LVT mode field is bits[18:17] (SDM Table 11-19), not the generic
/// delivery-mode field at bits[12:8] used by the LINT entries. Periodic = 01b.
const LVT_MODE_PERIODIC: u32 = 0b001 << 17;
const LVT_MASKED: u32 = 1 << 16;

/// ICR delivery modes and flags for inter-processor interrupts
const ICR_DELIVERY_INIT: u32 = 0b101 << 8;
const ICR_DELIVERY_STARTUP: u32 = 0b110 << 8;
const ICR_LEVEL_ASSERT: u32 = 1 << 14;

/// LAPIC timer divide configurations
#[allow(dead_code)] // full divider table kept for future timer-frequency tuning
const TDCR_DIV_BY_1: u32 = 0x0B;
const TDCR_DIV_BY_16: u32 = 0x03;
#[allow(dead_code)] // full divider table kept for future timer-frequency tuning
const TDCR_DIV_BY_256: u32 = 0x09;

/// Vector for the LAPIC timer LVT entry. Deliberately distinct from the
/// PIC-delivered timer vector (0x20) so the LAPIC timer can never fire into
/// the PIC timer gate. The entry stays masked until an IDT handler exists
/// for this vector and the timer is calibrated against the PIT.
pub const LAPIC_TIMER_VECTOR: u8 = 0x40;

/// Spurious-interrupt vector. Must have an IDT gate whose handler does not
/// send an EOI.
pub const LAPIC_SPURIOUS_VECTOR: u8 = 0xFF;

const SPURIOUS_VECTOR_ENABLE: u32 = 1 << 8;

/// LAPIC interrupt-controller driver
pub struct LapicTimer {
    base_address: u64,
    initialized: bool,
    armed: bool,
    ticks_per_ms: u32,
}

impl LapicTimer {
    /// Create a new LAPIC driver
    pub const fn new() -> Self {
        Self {
            base_address: DEFAULT_LAPIC_BASE,
            initialized: false,
            armed: false,
            ticks_per_ms: 0,
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

    /// Enable the local APIC as an interrupt controller (virtual-wire mode).
    ///
    /// Sets the global-enable bit in IA32_APIC_BASE (bit 11), routes external
    /// controller interrupts through LINT0 as ExtINT so the platform's
    /// external IRQ controller keeps working, software-enables the APIC via
    /// the spurious-interrupt vector register (bit 8), accepts all interrupt
    /// priorities by clearing TPR, and masks the LAPIC timer on its own
    /// distinct vector. The platform's reference tick source remains the
    /// system tick source until [`Self::calibrate_against_reference`] and
    /// [`Self::arm_periodic`] succeed, so the timer can never fire into the
    /// timer vector (0x20) while calibration is pending or failed.
    ///
    /// # Safety
    /// The IDT must already contain a spurious-interrupt handler for
    /// [`LAPIC_SPURIOUS_VECTOR`].
    pub unsafe fn initialize(&mut self) {
        if self.initialized {
            return;
        }

        // Globally enable the APIC via IA32_APIC_BASE and adopt the
        // MSR-provided MMIO base in case firmware relocated the APIC.
        let base = unsafe { crate::msr::read_msr(MSR_IA32_APIC_BASE) };
        let mmio_base = base & APIC_BASE_ADDR_MASK;
        if mmio_base != 0 {
            self.base_address = mmio_base;
        }
        if base & APIC_BASE_GLOBAL_ENABLE == 0 {
            unsafe {
                crate::msr::write_msr(MSR_IA32_APIC_BASE, base | APIC_BASE_GLOBAL_ENABLE);
            }
        }

        // Virtual-wire mode: deliver PIC interrupts through LINT0 as ExtINT.
        unsafe {
            self.write_reg(LAPIC_LVT_LINT0, LVT_MODE_EXTINT);
        }

        // Software-enable the APIC, accept all interrupt priorities, and keep
        // the LAPIC timer entry masked on its distinct vector.
        unsafe {
            self.write_reg(
                LAPIC_SPURIOUS,
                SPURIOUS_VECTOR_ENABLE | LAPIC_SPURIOUS_VECTOR as u32,
            );
            self.write_reg(LAPIC_TPR, 0);
            self.write_reg(LAPIC_LVT_TIMER, LVT_MASKED | LAPIC_TIMER_VECTOR as u32);
            self.write_reg(LAPIC_TIMER_DCR, TDCR_DIV_BY_16);
        }

        self.initialized = true;
    }

    /// Measure the LAPIC counter frequency against the platform's reference
    /// tick source over a short window and return the counter ticks elapsed
    /// per millisecond.
    ///
    /// The reference source must already be running in periodic mode at
    /// `tick_hz`. The LAPIC timer is started at its maximum initial count
    /// with a divide value of 16 while this function busy-polls the source
    /// for `ref_ticks` full period wraps. On any timeout or nonsensical
    /// reading the timer is stopped again and `None` is returned so callers
    /// stay on the reference source.
    pub fn calibrate_against_reference(&mut self, tick_hz: u32, ref_ticks: u32) -> Option<u32> {
        if !self.initialized || ref_ticks == 0 {
            return None;
        }

        unsafe {
            self.write_reg(LAPIC_LVT_TIMER, LVT_MASKED | LAPIC_TIMER_VECTOR as u32);
            self.write_reg(LAPIC_TIMER_DCR, TDCR_DIV_BY_16);
            self.write_reg(LAPIC_TIMER_ICR, u32::MAX);
        }
        let start_count = unsafe { self.read_reg(LAPIC_TIMER_CCR) };

        if !(super::interrupts::external_irq_ops().wait_tick_wraps)(ref_ticks) {
            unsafe { self.write_reg(LAPIC_TIMER_ICR, 0) };
            return None;
        }

        let end_count = unsafe { self.read_reg(LAPIC_TIMER_CCR) };
        unsafe { self.write_reg(LAPIC_TIMER_ICR, 0) };

        // The counter only ever counts down from the maximum we programmed,
        // so a plain subtraction measures the elapsed ticks.
        let elapsed = start_count.wrapping_sub(end_count) as u64;
        let window_ms = (ref_ticks as u64 * 1000) / tick_hz.max(1) as u64;
        if window_ms == 0 || elapsed < 1000 {
            return None;
        }
        Some((elapsed / window_ms) as u32)
    }

    /// Arm the LAPIC timer in periodic mode on [`LAPIC_TIMER_VECTOR`] with a
    /// period of one system tick (`tick_hz` periods per second).
    ///
    /// # Safety
    /// An IDT gate for [`LAPIC_TIMER_VECTOR`] must route to the timer IRQ
    /// entry stub before this is called.
    pub unsafe fn arm_periodic(&mut self, tick_hz: u32, ticks_per_ms: u32) {
        if !self.initialized || ticks_per_ms == 0 {
            return;
        }
        let ticks_per_period = (ticks_per_ms as u64 * 1000 / tick_hz.max(1) as u64) as u32;
        if ticks_per_period == 0 {
            return;
        }

        unsafe {
            self.write_reg(
                LAPIC_LVT_TIMER,
                LVT_MODE_PERIODIC | LAPIC_TIMER_VECTOR as u32,
            );
            self.write_reg(LAPIC_TIMER_DCR, TDCR_DIV_BY_16);
            self.start(ticks_per_period);
        }
        self.ticks_per_ms = ticks_per_ms;
        self.armed = true;
    }

    /// Whether the LAPIC timer is armed as the system tick source
    pub fn is_armed(&self) -> bool {
        self.armed
    }

    /// This CPU's local APIC ID (0 when the LAPIC is uninitialized)
    pub fn current_apic_id(&self) -> u8 {
        if !self.initialized {
            return 0;
        }
        (unsafe { self.read_reg(LAPIC_ID) } >> 24) as u8
    }

    /// Send an INIT IPI asserting reset semantics to one APIC ID.
    ///
    /// # Safety
    /// The target APIC must exist; callers own the bring-up sequence.
    pub unsafe fn send_init_ipi(&self, apic_id: u8) {
        unsafe {
            self.write_reg(LAPIC_ICR_HIGH, (apic_id as u32) << 24);
            // Edge-triggered INIT assert: the classic bring-up form that
            // needs no follow-up level-deassert write.
            self.write_reg(LAPIC_ICR_LOW, ICR_DELIVERY_INIT);
        }
    }

    /// Send a Start-Up IPI directing the target to begin at `vector * 4KiB`.
    ///
    /// # Safety
    /// The vector page must contain valid real-mode code; callers own the
    /// bring-up sequence.
    pub unsafe fn send_startup_ipi(&self, apic_id: u8, vector: u8) {
        unsafe {
            self.write_reg(LAPIC_ICR_HIGH, (apic_id as u32) << 24);
            self.write_reg(
                LAPIC_ICR_LOW,
                ICR_DELIVERY_STARTUP | ICR_LEVEL_ASSERT | vector as u32,
            );
        }
    }

    /// Calibrated counter ticks per millisecond (0 before calibration)
    pub fn ticks_per_ms(&self) -> u32 {
        self.ticks_per_ms
    }

    /// Arm the LAPIC timer (no effect while the LVT entry is masked)
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

    /// Check if the LAPIC is initialized
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }
}

/// Global LAPIC driver instance
static LAPIC_TIMER: Mutex<LapicTimer> = Mutex::new(LapicTimer::new());

/// Enable the LAPIC as an interrupt controller
///
/// # Safety
/// See [`LapicTimer::initialize`].
pub unsafe fn initialize() {
    unsafe {
        LAPIC_TIMER.lock().initialize();
    }
}

/// Get a reference to the global LAPIC driver
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

/// This CPU's local APIC ID (0 before LAPIC initialization)
pub fn current_apic_id() -> u8 {
    LAPIC_TIMER.lock().current_apic_id()
}

/// Send an INIT IPI to one APIC ID
///
/// # Safety
/// See [`LapicTimer::send_init_ipi`].
pub unsafe fn send_init_ipi(apic_id: u8) {
    unsafe {
        LAPIC_TIMER.lock().send_init_ipi(apic_id);
    }
}

/// Send a Start-Up IPI targeting `vector * 4KiB` on one APIC ID
///
/// # Safety
/// See [`LapicTimer::send_startup_ipi`].
pub unsafe fn send_startup_ipi(apic_id: u8, vector: u8) {
    unsafe {
        LAPIC_TIMER.lock().send_startup_ipi(apic_id, vector);
    }
}
