//! Machine-level constants and the QEMU sifive_test finisher.

pub const FINISHER_PASS: u32 = 0x5555;
pub const FINISHER_FAIL: u32 = 0x3333;
pub const FINISHER_RESET: u32 = 0x7777;

#[cfg(target_arch = "riscv64")]
mod imp {
    use serviceos_kernel_arch_riscv64::layout;

    pub fn qemu_exit(code: u32) -> ! {
        unsafe {
            core::ptr::write_volatile(layout::TEST_DEVICE_BASE as *mut u32, code.to_le());
        }
        unreachable!("sifive_test finisher did not terminate QEMU")
    }
}

#[cfg(not(target_arch = "riscv64"))]
mod imp {
    pub fn qemu_exit(_code: u32) -> ! {
        unreachable!("sifive_test finisher is only available on riscv64 targets")
    }
}

pub use imp::qemu_exit;
