use core::arch::asm;

/// Per-CPU data structure for SYSCALL/SYSRET fast path
///
/// This structure is accessed via the GS base register and provides
/// fast access to per-CPU state during syscall entry/exit.
#[repr(C)]
#[derive(Copy, Clone)]
pub struct PerCpuData {
    /// Kernel stack pointer (offset 0x00)
    pub kernel_rsp: u64,
    /// User stack pointer (offset 0x08)
    pub user_rsp: u64,
    /// Current thread ID (offset 0x10)
    pub current_thread: u64,
    /// CPU ID (offset 0x18)
    pub cpu_id: u64,
}

/// Maximum number of CPUs supported
pub const MAX_CPUS: usize = 64;

/// Per-CPU data storage
static mut PER_CPU_DATA: [PerCpuData; MAX_CPUS] = [PerCpuData {
    kernel_rsp: 0,
    user_rsp: 0,
    current_thread: 0,
    cpu_id: 0,
}; MAX_CPUS];

/// Initialize per-CPU data for the current CPU
///
/// # Safety
/// This function must be called once per CPU during initialization
/// with interrupts disabled.
pub unsafe fn initialize_per_cpu_data(cpu_id: usize, kernel_rsp: u64) {
    if cpu_id >= MAX_CPUS {
        panic!("CPU ID {} exceeds maximum supported CPUs", cpu_id);
    }

    unsafe {
        PER_CPU_DATA[cpu_id].kernel_rsp = kernel_rsp;
        PER_CPU_DATA[cpu_id].user_rsp = 0;
        PER_CPU_DATA[cpu_id].current_thread = 0;
        PER_CPU_DATA[cpu_id].cpu_id = cpu_id as u64;

        // Set GS base to point to this CPU's per-CPU data
        let gs_base = &PER_CPU_DATA[cpu_id] as *const PerCpuData as u64;
        write_msr(0xC000_0101, gs_base); // MSR_GS_BASE
    }
}

/// Read a Model-Specific Register
unsafe fn write_msr(msr: u32, value: u64) {
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

/// Get the current CPU's per-CPU data
///
/// # Safety
/// This function assumes GS base is properly initialized for the current CPU.
pub unsafe fn current_cpu_data() -> &'static mut PerCpuData {
    let gs_base: u64;
    unsafe {
        asm!("mov {}, gs:0x00", out(reg) gs_base);
        &mut *(gs_base as *mut PerCpuData)
    }
}

/// Update the kernel stack pointer for the current CPU
///
/// # Safety
/// This function modifies per-CPU state and should only be called
/// during context switch operations.
pub unsafe fn update_kernel_rsp(rsp: u64) {
    unsafe {
        asm!("mov gs:0x00, {}", in(reg) rsp);
    }
}

/// Get the user stack pointer for the current CPU
pub fn user_rsp() -> u64 {
    unsafe { current_cpu_data().user_rsp }
}

/// Get the current thread ID for the current CPU
pub fn current_thread_id() -> u64 {
    unsafe { current_cpu_data().current_thread }
}

/// Set the current thread ID for the current CPU
///
/// # Safety
/// This function modifies per-CPU state and should only be called
/// during context switch operations.
pub unsafe fn set_current_thread_id(thread_id: u64) {
    unsafe {
        current_cpu_data().current_thread = thread_id;
    }
}
