#[cfg(target_arch = "aarch64")]
mod imp {
    use core::arch::asm;

    pub fn disable_interrupts() {
        unsafe {
            asm!("msr daifset, #15", options(nomem, nostack, preserves_flags));
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

    pub fn core_id() -> u64 {
        let mpidr: u64;
        unsafe {
            asm!("mrs {value}, MPIDR_EL1", value = out(reg) mpidr, options(nomem, nostack, preserves_flags));
        }
        mpidr & 0xff
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
    core_id, current_el, data_synchronization_barrier, disable_interrupts, enable_interrupts,
    instruction_synchronization_barrier, wait_for_event,
};

pub fn wait_forever() -> ! {
    loop {
        wait_for_event();
    }
}
