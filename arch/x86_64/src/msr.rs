use core::arch::asm;

/// Model-Specific Register addresses for SYSCALL/SYSRET
pub const MSR_STAR: u32 = 0xC000_0080;
pub const MSR_LSTAR: u32 = 0xC000_0081;
pub const MSR_FMASK: u32 = 0xC000_0084;
pub const MSR_EFER: u32 = 0xC000_0080;

/// EFER bits
pub const EFER_SCE: u64 = 1 << 0;

/// Read a Model-Specific Register
pub unsafe fn read_msr(msr: u32) -> u64 {
    let low: u32;
    let high: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") msr,
            out("eax") low,
            out("edx") high,
        );
    }
    ((high as u64) << 32) | (low as u64)
}

/// Write a Model-Specific Register
pub unsafe fn write_msr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    unsafe {
        asm!(
            "wrmsr",
            in("ecx") msr,
            in("eax") low,
            in("edx") high,
        );
    }
}

/// Enable SYSCALL/SYSRET support
///
/// # Safety
/// This function modifies CPU MSRs and must be called during initialization
/// with interrupts disabled.
pub unsafe fn enable_syscall_sysret(
    kernel_entry: u64,
    kernel_cs: u16,
    user_cs: u16,
    user_ss: u16,
) {
    // STAR MSR layout:
    // - Bits [47:32]: CS for SYSCALL (kernel CS = kernel_cs)
    // - Bits [31:16]: CS for SYSRET (user CS = user_cs)
    // - Bits [15:0]:  SS for SYSRET (user SS = user_ss)
    let star_value = ((kernel_cs as u64) << 48)
        | ((user_cs as u64) << 32)
        | ((user_cs as u64) << 16)
        | (user_ss as u64);

    unsafe {
        write_msr(MSR_STAR, star_value);
        write_msr(MSR_LSTAR, kernel_entry);
        write_msr(MSR_FMASK, 0x200); // Mask IF on SYSCALL entry
    }

    // Enable SCE (System Call Extensions) in EFER
    let efer = unsafe { read_msr(MSR_EFER) };
    unsafe {
        write_msr(MSR_EFER, efer | EFER_SCE);
    }
}
