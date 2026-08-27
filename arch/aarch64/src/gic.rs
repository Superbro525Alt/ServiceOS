use core::sync::atomic::{AtomicU16, Ordering};
use serviceos_kernel_core::memory::PhysicalAddress;

/// Default INTID for the guest-visible EL1 physical timer. The driver arms
/// `cntp_cval_el0`, which asserts CNTPNSIRQ = INTID 30 (PPI 14 in the device
/// tree). The secure physical timer PPI 13 (INTID 29) never fires at
/// non-secure EL1, so enabling it silently swallowed every timer interrupt.
pub const DEFAULT_TIMER_PPI_INTID: u16 = 30;
const SPURIOUS_INTID_MIN: u16 = 1020;
const INTID_FIELD_MASK: u64 = 0x3ff;

static TIMER_PPI_INTID: AtomicU16 = AtomicU16::new(DEFAULT_TIMER_PPI_INTID);

pub fn set_timer_ppi_intid(intid: u16) {
    TIMER_PPI_INTID.store(intid, Ordering::Relaxed);
}

pub fn timer_ppi_intid() -> u16 {
    TIMER_PPI_INTID.load(Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GicConfig {
    pub distributor_base: PhysicalAddress,
    pub redistributor_base: PhysicalAddress,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GicInitError {
    Unavailable,
    MisalignedRegion,
    RedistributorWakeTimeout,
    DistributorWriteTimeout,
    SystemRegisterUnsupported,
}

pub const fn decode_intid(interrupt_acknowledge_register: u64) -> u16 {
    (interrupt_acknowledge_register & INTID_FIELD_MASK) as u16
}

pub const fn is_spurious(intid: u16) -> bool {
    intid >= SPURIOUS_INTID_MIN
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AcknowledgedIrq {
    pub intid: u16,
    raw: u64,
}

impl AcknowledgedIrq {
    pub const fn from_raw(raw: u64) -> Self {
        Self {
            intid: decode_intid(raw),
            raw,
        }
    }

    pub const fn raw(self) -> u64 {
        self.raw
    }
}

#[cfg(target_arch = "aarch64")]
mod imp {
    use core::arch::asm;
    use core::ptr;

    use spin::Once;

    use super::*;

    const REDISTRIBUTOR_SGI_FRAME_OFFSET: u64 = 64 * 1024;

    const GICD_CTLR: u64 = 0x0000;
    const GICD_TYPER: u64 = 0x0004;
    const GICD_IGROUPR: u64 = 0x0080;
    const GICD_ISENABLER: u64 = 0x0100;
    const GICD_ICENABLER: u64 = 0x0180;
    const GICD_IPRIORITYR: u64 = 0x0400;
    const GICD_ICFGR: u64 = 0x0c00;

    const GICR_WAKER: u64 = 0x0014;

    const CTLR_ENABLE_GRP1_NS: u32 = 1 << 1;
    const CTLR_RWP: u32 = 1 << 31;
    const WAKER_PROCESSOR_SLEEP: u32 = 1 << 1;
    const WAKER_CHILDREN_ASLEEP: u32 = 1 << 2;

    const TIMER_PPI_PRIORITY: u32 = 0xa0;
    const REGISTER_WRITE_POLL_LIMIT: usize = 1_000_000;

    static ACTIVE_GIC: Once<GicConfig> = Once::new();

    pub fn is_active() -> bool {
        ACTIVE_GIC.get().is_some()
    }

    fn read_register(base: PhysicalAddress, offset: u64) -> u32 {
        unsafe { ptr::read_volatile((base.as_u64() + offset) as *const u32) }
    }

    fn write_register(base: PhysicalAddress, offset: u64, value: u32) {
        unsafe { ptr::write_volatile((base.as_u64() + offset) as *mut u32, value) }
    }

    fn distributor_wait_rwp(base: PhysicalAddress) -> bool {
        for _ in 0..REGISTER_WRITE_POLL_LIMIT {
            if read_register(base, GICD_CTLR) & CTLR_RWP == 0 {
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }

    fn wake_redistributor(base: PhysicalAddress) -> Result<(), GicInitError> {
        let waker = read_register(base, GICR_WAKER);
        write_register(base, GICR_WAKER, waker & !WAKER_PROCESSOR_SLEEP);
        for _ in 0..REGISTER_WRITE_POLL_LIMIT {
            if read_register(base, GICR_WAKER) & WAKER_CHILDREN_ASLEEP == 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(GicInitError::RedistributorWakeTimeout)
    }

    fn configure_sgi_ppi_frame(base: PhysicalAddress) {
        let timer_intid = u32::from(super::timer_ppi_intid());
        let sgi_base = PhysicalAddress::new(base.as_u64() + REDISTRIBUTOR_SGI_FRAME_OFFSET);
        write_register(sgi_base, GICD_ICENABLER, u32::MAX);
        write_register(sgi_base, GICD_IGROUPR, u32::MAX);
        let priority_register_index = timer_intid / 4;
        let priority_shift = (timer_intid % 4) * 8;
        let priority_address = GICD_IPRIORITYR + u64::from(priority_register_index) * 4;
        write_register(
            sgi_base,
            priority_address,
            TIMER_PPI_PRIORITY << priority_shift,
        );
        write_register(sgi_base, GICD_ISENABLER, 1 << (timer_intid % 32));
    }

    fn configure_spi_defaults(base: PhysicalAddress) {
        let typer = read_register(base, GICD_TYPER);
        let itlines = u64::from(typer & 0x1f);
        for line in 1..=itlines {
            let offset = line * 4;
            write_register(base, GICD_ICENABLER + offset, u32::MAX);
            write_register(base, GICD_IGROUPR + offset, u32::MAX);
            write_register(base, GICD_ICFGR + offset, 0);
        }
    }

    fn configure_system_registers() -> Result<(), GicInitError> {
        let sre: u64;
        unsafe {
            asm!("mrs {}, icc_sre_el1", out(reg) sre, options(nostack));
        }
        unsafe {
            asm!(
                "msr icc_sre_el1, {value}",
                value = in(reg) sre | 0b111,
                options(nostack)
            );
        }
        let verified: u64;
        unsafe {
            asm!("mrs {}, icc_sre_el1", out(reg) verified, options(nostack));
        }
        if verified & 0b1 == 0 {
            return Err(GicInitError::SystemRegisterUnsupported);
        }

        unsafe {
            asm!("msr icc_pmr_el1, {value}", value = in(reg) 0xffu64, options(nostack));
            asm!("msr icc_igrpen1_el1, {value}", value = in(reg) 1u64, options(nostack));
            asm!("dsb sy", options(nostack));
            asm!("isb", options(nostack));
        }
        Ok(())
    }

    pub fn initialize(config: GicConfig) -> Result<(), GicInitError> {
        if config.distributor_base.as_u64() & 0x3 != 0
            || config.redistributor_base.as_u64() & 0x3 != 0
        {
            return Err(GicInitError::MisalignedRegion);
        }

        write_register(config.distributor_base, GICD_CTLR, 0);
        if !distributor_wait_rwp(config.distributor_base) {
            return Err(GicInitError::DistributorWriteTimeout);
        }

        wake_redistributor(config.redistributor_base)?;
        configure_sgi_ppi_frame(config.redistributor_base);
        configure_spi_defaults(config.distributor_base);

        write_register(config.distributor_base, GICD_CTLR, CTLR_ENABLE_GRP1_NS);
        if !distributor_wait_rwp(config.distributor_base) {
            return Err(GicInitError::DistributorWriteTimeout);
        }

        configure_system_registers()?;
        ACTIVE_GIC.call_once(|| config);
        Ok(())
    }

    pub fn acknowledge() -> Option<AcknowledgedIrq> {
        ACTIVE_GIC.get()?;
        let iar: u64;
        unsafe {
            asm!("mrs {}, icc_iar1_el1", out(reg) iar, options(nostack));
        }
        let irq = AcknowledgedIrq::from_raw(iar);
        if is_spurious(irq.intid) {
            return None;
        }
        Some(irq)
    }

    /// Enable one SPI in the distributor so a device interrupt can actually
    /// reach the redistributor. `configure_spi_defaults` disables every SPI
    /// during init, so each virtio-mmio device INTID must be enabled here.
    pub fn enable_spi(intid: u16) {
        let Some(config) = ACTIVE_GIC.get() else {
            return;
        };
        if intid < 32 {
            return;
        }
        let index = u64::from(intid) / 32;
        let offset = GICD_ISENABLER + index * 4;
        let bit = 1u32 << (u64::from(intid) % 32);
        write_register(config.distributor_base, offset, bit);
    }

    pub fn end_of_interrupt(irq: AcknowledgedIrq) {
        if !ACTIVE_GIC.get().is_some() {
            return;
        }
        unsafe {
            asm!(
                "msr icc_eoir1_el1, {value}",
                value = in(reg) irq.raw(),
                options(nostack)
            );
        }
    }
}

#[cfg(not(target_arch = "aarch64"))]
mod imp {
    use super::*;

    pub fn is_active() -> bool {
        false
    }

    pub fn initialize(_config: GicConfig) -> Result<(), GicInitError> {
        Err(GicInitError::Unavailable)
    }

    pub fn acknowledge() -> Option<AcknowledgedIrq> {
        None
    }

    pub fn enable_spi(_intid: u16) {}

    pub fn end_of_interrupt(_irq: AcknowledgedIrq) {}
}

pub use imp::{acknowledge, enable_spi, end_of_interrupt, initialize, is_active};
