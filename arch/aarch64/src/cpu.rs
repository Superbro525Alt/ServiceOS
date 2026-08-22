#[cfg(target_arch = "aarch64")]
mod imp {
    use core::arch::asm;

    pub fn disable_interrupts() {
        unsafe {
            asm!("msr daifset, #15", options(nomem, nostack, preserves_flags));
        }
    }

    pub fn enable_irqs() {
        unsafe {
            asm!("msr daifclr, #2", options(nomem, nostack, preserves_flags));
        }
    }

    pub fn disable_irqs() {
        unsafe {
            asm!("msr daifset, #2", options(nomem, nostack, preserves_flags));
        }
    }

    pub fn wait_for_interrupt() {
        unsafe {
            asm!("wfi", options(nomem, nostack));
        }
    }

    pub fn enable_interrupts() {
        unsafe {
            asm!("msr daifclr, #15", options(nomem, nostack, preserves_flags));
        }
    }

    pub fn current_el() -> u8 {
        let current_el: u64;
        unsafe {
            asm!("mrs {value}, CurrentEL", value = out(reg) current_el, options(nomem, nostack, preserves_flags));
        }
        ((current_el >> 2) & 0b11) as u8
    }

    pub fn core_id() -> u32 {
        let mpidr: u64;
        unsafe {
            asm!(
                "mrs {value}, MPIDR_EL1",
                value = out(reg) mpidr,
                options(nomem, nostack, preserves_flags)
            );
        }

        let aff0 = (mpidr & 0xff) as u32;
        let aff1 = ((mpidr >> 8) & 0xff) as u32;
        let aff2 = ((mpidr >> 16) & 0xff) as u32;
        let aff3 = ((mpidr >> 32) & 0xff) as u32;

        aff0 | (aff1 << 8) | (aff2 << 16) | (aff3 << 24)
    }

    pub fn data_synchronization_barrier() {
        unsafe {
            asm!("dsb sy", options(nomem, nostack, preserves_flags));
        }
    }

    pub fn instruction_synchronization_barrier() {
        unsafe {
            asm!("isb", options(nomem, nostack, preserves_flags));
        }
    }

    pub fn wait_for_event() {
        unsafe {
            asm!("wfe", options(nomem, nostack));
        }
    }
}

#[cfg(not(target_arch = "aarch64"))]
mod imp {
    pub fn disable_interrupts() {}

    pub fn enable_irqs() {}

    pub fn disable_irqs() {}

    pub fn wait_for_interrupt() {
        core::hint::spin_loop();
    }

    pub fn enable_interrupts() {}

    pub fn current_el() -> u8 {
        0
    }

    pub fn core_id() -> u64 {
        0
    }

    pub fn data_synchronization_barrier() {}

    pub fn instruction_synchronization_barrier() {}

    pub fn wait_for_event() {
        core::hint::spin_loop();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CpuBringupStatus {
    pub early_entry: bool,
    pub interrupt_masking: bool,
    pub wait_loops: bool,
}

pub const fn bringup_status() -> CpuBringupStatus {
    CpuBringupStatus {
        early_entry: true,
        interrupt_masking: true,
        wait_loops: true,
    }
}

pub use imp::{
    core_id, current_el, data_synchronization_barrier, disable_interrupts, disable_irqs,
    enable_interrupts, enable_irqs, instruction_synchronization_barrier, wait_for_event,
    wait_for_interrupt,
};

pub fn wait_forever() -> ! {
    loop {
        wait_for_event();
    }
}
