use core::arch::asm;

/// Model-Specific Register addresses for SYSCALL/SYSRET
pub const MSR_EFER: u32 = 0xC000_0080;
pub const MSR_STAR: u32 = 0xC000_0081;
pub const MSR_LSTAR: u32 = 0xC000_0082;
pub const MSR_FMASK: u32 = 0xC000_0084;

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
    _user_ss: u16,
) {
    // IA32_STAR layout (SDM Vol 3, 5.8.7):
    // - Bits [63:48]: base selector SYSRET uses for the user code segment
    //   (SS is derived as +8, so the GDT must place user data directly after
    //   user code).
    // - Bits [47:32]: selector SYSCALL loads into CS; SS = this + 8, so the
    //   kernel data segment must follow the kernel code segment.
    // - Bits [31:0]: reserved, must be zero.
    let star_value = ((user_cs as u64) << 48) | ((kernel_cs as u64) << 32);

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
