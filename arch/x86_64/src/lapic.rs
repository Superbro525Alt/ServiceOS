use core::arch::asm;
use spin::Mutex;

/// Default local APIC MMIO base; overridden by IA32_APIC_BASE if firmware relocated the APIC.
const DEFAULT_LAPIC_BASE: u64 = 0x0000_0000_FEE0_0000;

/// IA32_APIC_BASE MSR and its fields
const MSR_IA32_APIC_BASE: u32 = 0x1B;
const APIC_BASE_ADDR_MASK: u64 = 0xFFFF_FFFF_FFFF_F000;
const APIC_BASE_GLOBAL_ENABLE: u64 = 1 << 11;

/// LAPIC register offsets
const LAPIC_TPR: u32 = 0x080;
const LAPIC_EOI: u32 = 0x0B0;
const LAPIC_SPURIOUS: u32 = 0x0F0;
const LAPIC_LVT_TIMER: u32 = 0x320;
const LAPIC_LVT_LINT0: u32 = 0x350;
const LAPIC_TIMER_ICR: u32 = 0x380;
const LAPIC_TIMER_DCR: u32 = 0x3E0;

/// LAPIC LVT entry bits
const LVT_MODE_EXTINT: u32 = 0b111 << 8;
const LVT_MASKED: u32 = 1 << 16;

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
}

impl LapicTimer {
    /// Create a new LAPIC driver
    pub const fn new() -> Self {
        Self {
            base_address: DEFAULT_LAPIC_BASE,
            initialized: false,
        }
    }

    /// Read a LAPIC register
    #[allow(dead_code)] // read counterpart of write_reg; needed for future timer calibration (CCR/ICR reads)
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
    /// Sets the global-enable bit in IA32_APIC_BASE (bit 11), routes PIC
    /// interrupts through LINT0 as ExtINT so the 8259 PIC keeps working,
    /// software-enables the APIC via the spurious-interrupt vector register
    /// (bit 8), accepts all interrupt priorities by clearing TPR, and masks
    /// the LAPIC timer on its own distinct vector. The PIT/PIC remains the
    /// system tick source; LAPIC timer calibration against the PIT is still
    /// TODO, so the timer is never armed here and can never fire on the PIC
    /// timer vector (0x20).
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
