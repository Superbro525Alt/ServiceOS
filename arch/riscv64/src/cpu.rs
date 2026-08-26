//! Hart control helpers: park loops for secondary harts and halted waits.

#[cfg(target_arch = "riscv64")]
pub fn park() -> ! {
    loop {
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack));
        }
    }
}

#[cfg(not(target_arch = "riscv64"))]
pub fn park() -> ! {
    unreachable!("riscv64 hart park is only available on riscv64 targets")
}
