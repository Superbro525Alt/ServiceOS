//! SBI call wrappers (RISC-V Supervisor Binary Interface).
//!
//! Console output uses the legacy `console_putchar` call (EID 0x01), which
//! every mainstream firmware including OpenSBI provides. Timer scheduling
//! uses the modern TIME extension (EID 0x54494D45, FID 0).


pub const SBI_EXT_LEGACY_CONSOLE_PUTCHAR: usize = 0x01;
pub const SBI_EXT_TIME: usize = 0x54494D45;
pub const SBI_EXT_TIME_SET_TIMER: usize = 0x0;

#[cfg(target_arch = "riscv64")]
fn ecall(extension: usize, function: usize, arg0: usize) -> (usize, usize) {
    let error: usize;
    let value: usize;
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") arg0 => error,
            lateout("a1") value,
            in("a6") function,
            in("a7") extension,
            options(nomem, nostack)
        );
    }
    (error, value)
}

#[cfg(target_arch = "riscv64")]
pub fn console_putchar(byte: u8) {
    ecall(SBI_EXT_LEGACY_CONSOLE_PUTCHAR, 0, byte as usize);
}

#[cfg(target_arch = "riscv64")]
pub fn set_timer(stime_value: u64) {
    let value = stime_value as usize;
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") value => _,
            lateout("a1") _,
            in("a6") SBI_EXT_TIME_SET_TIMER,
            in("a7") SBI_EXT_TIME,
            options(nomem, nostack)
        );
    }
}

#[cfg(not(target_arch = "riscv64"))]
pub fn console_putchar(_byte: u8) {}

#[cfg(not(target_arch = "riscv64"))]
pub fn set_timer(_stime_value: u64) {}
