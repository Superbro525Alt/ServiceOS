//! Supervisor trap-vector setup.
//!
//! Skeleton scope: one all-traps entry point. Every trap prints its
//! scause/sepc/stval and then hangs (wfi loop) so failures are visible on
//! serial instead of silently spinning. No trap dispatch, no userspace
//! trapframes, no nested/interrupt policy yet.

#[cfg(target_arch = "riscv64")]
#[repr(C)]
pub struct TrapFrame {
    pub regs: [usize; 31],
    pub scause: usize,
    pub stval: usize,
    pub sepc: usize,
}

#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(
    ".section .text.trap, \"ax\"",
    ".globl riscv_all_traps",
    ".align 2",
    "riscv_all_traps:",
    "addi sp, sp, -{frame_size}",
    "sd x1, {ra}(sp)",
    "sd x2, {sp}(sp)",
    "sd x3, 3*8(sp)",
    "sd x4, 4*8(sp)",
    "sd x5, 5*8(sp)",
    "sd x6, 6*8(sp)",
    "sd x7, 7*8(sp)",
    "sd x8, 8*8(sp)",
    "sd x9, 9*8(sp)",
    "sd x10, 10*8(sp)",
    "sd x11, 11*8(sp)",
    "sd x12, 12*8(sp)",
    "sd x13, 13*8(sp)",
    "sd x14, 14*8(sp)",
    "sd x15, 15*8(sp)",
    "sd x16, 16*8(sp)",
    "sd x17, 17*8(sp)",
    "sd x18, 18*8(sp)",
    "sd x19, 19*8(sp)",
    "sd x20, 20*8(sp)",
    "sd x21, 21*8(sp)",
    "sd x22, 22*8(sp)",
    "sd x23, 23*8(sp)",
    "sd x24, 24*8(sp)",
    "sd x25, 25*8(sp)",
    "sd x26, 26*8(sp)",
    "sd x27, 27*8(sp)",
    "sd x28, 28*8(sp)",
    "sd x29, 29*8(sp)",
    "sd x30, 30*8(sp)",
    "sd x31, 31*8(sp)",
    "csrr t0, scause",
    "csrr t1, stval",
    "csrr t2, sepc",
    "sd t0, 32*8(sp)",
    "sd t1, 33*8(sp)",
    "sd t2, 34*8(sp)",
    "mv a0, sp",
    "call riscv_trap_handler",
    "ld t0, 32*8(sp)",
    "ld t1, 33*8(sp)",
    "ld t2, 34*8(sp)",
    "csrw scause, t0",
    "csrw stval, t1",
    "csrw sepc, t2",
    "ld x1, {ra}(sp)",
    "ld x3, 3*8(sp)",
    "ld x4, 4*8(sp)",
    "ld x5, 5*8(sp)",
    "ld x6, 6*8(sp)",
    "ld x7, 7*8(sp)",
    "ld x8, 8*8(sp)",
    "ld x9, 9*8(sp)",
    "ld x10, 10*8(sp)",
    "ld x11, 11*8(sp)",
    "ld x12, 12*8(sp)",
    "ld x13, 13*8(sp)",
    "ld x14, 14*8(sp)",
    "ld x15, 15*8(sp)",
    "ld x16, 16*8(sp)",
    "ld x17, 17*8(sp)",
    "ld x18, 18*8(sp)",
    "ld x19, 19*8(sp)",
    "ld x20, 20*8(sp)",
    "ld x21, 21*8(sp)",
    "ld x22, 22*8(sp)",
    "ld x23, 23*8(sp)",
    "ld x24, 24*8(sp)",
    "ld x25, 25*8(sp)",
    "ld x26, 26*8(sp)",
    "ld x27, 27*8(sp)",
    "ld x28, 28*8(sp)",
    "ld x29, 29*8(sp)",
    "ld x30, 30*8(sp)",
    "ld x31, 31*8(sp)",
    "ld x2, {sp}(sp)",
    "addi sp, sp, {frame_size}",
    "sret",
    ".section .text",
    frame_size = const 35 * 8,
    ra = const 1 * 8,
    sp = const 2 * 8,
);

#[cfg(target_arch = "riscv64")]
pub fn init() {
    use core::arch::asm;

    unsafe {
        asm!(
            "la {vector}, riscv_all_traps",
            "csrw stvec, {vector}",
            vector = out(reg) _,
            options(nomem, nostack)
        );
    }
}

#[cfg(target_arch = "riscv64")]
#[unsafe(no_mangle)]
extern "C" fn riscv_trap_handler(frame: &TrapFrame) {
    use crate::{cpu, sbi_println};

    sbi_println!(
        "serviceos: trap cause={:#x} sepc={:#x} stval={:#x} (all-traps hang)",
        frame.scause,
        frame.sepc,
        frame.stval
    );
    cpu::park();
}

#[cfg(not(target_arch = "riscv64"))]
pub fn init() {}
