use core::arch::asm;

/// Kernel thread context saved during context switch
///
/// This structure holds the callee-saved registers that must be preserved
/// when switching between kernel threads.
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct KernelContext {
    /// Stack pointer
    pub rsp: u64,
    /// Callee-saved registers
    pub rbx: u64,
    pub rbp: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
}

impl KernelContext {
    /// Create a new kernel context for a thread starting at the given function
    ///
    /// # Safety
    /// The function pointer must be valid and the stack must be properly allocated.
    pub unsafe fn new(entry: extern "C" fn(*mut u8), stack_top: *mut u8) -> Self {
        let mut ctx = Self {
            rsp: 0,
            rbx: 0,
            rbp: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
        };

        // Set up the initial stack frame for the thread
        // The stack should look like: [return address] [argument]
        unsafe {
            let rsp = stack_top as u64;
            // Push the entry function as a return address
            let rsp = rsp - 8;
            *(rsp as *mut u64) = entry as u64;
            // Push the argument
            let rsp = rsp - 8;
            *(rsp as *mut u64) = core::ptr::null_mut::<u8>() as u64;
            ctx.rsp = rsp;
        }

        ctx
    }
}

/// Switch from one kernel thread context to another
///
/// # Safety
/// This function must be called with interrupts disabled and the
/// scheduler lock held. The caller must ensure proper synchronization.
pub unsafe fn kernel_context_switch(from: &mut KernelContext, to: &KernelContext) {
    unsafe {
        asm!(
            // Save current context
            "mov {from}.rsp, rsp",
            "mov {from}.rbx, rbx",
            "mov {from}.rbp, rbp",
            "mov {from}.r12, r12",
            "mov {from}.r13, r13",
            "mov {from}.r14, r14",
            "mov {from}.r15, r15",
            
            // Restore next context
            "mov rsp, {to}.rsp",
            "mov rbx, {to}.rbx",
            "mov rbp, {to}.rbp",
            "mov r12, {to}.r12",
            "mov r13, {to}.r13",
            "mov r14, {to}.r14",
            "mov r15, {to}.r15",
            
            // The return address is on the stack, so ret will jump to it
            "ret",
            
            from = in(reg) from as *mut KernelContext,
            to = in(reg) to as *const KernelContext,
        );
    }
}

/// Initialize a kernel thread's context for the first run
///
/// This sets up the context so that when `kernel_context_switch` is called
/// to switch to this thread, it will start executing at the given entry point.
pub fn init_kernel_thread_context(
    ctx: &mut KernelContext,
    entry: u64,
    stack_top: u64,
    arg: u64,
) {
    // Set up a minimal stack frame that looks like we're returning from a function call
    // Stack layout (growing downward):
    // [arg]           <- stack_top - 8
    // [return addr]   <- stack_top - 16 (entry point)
    // [rbp]           <- stack_top - 24 (initial rbp = 0)
    let rsp = stack_top - 24;
    unsafe {
        // Push the entry point as return address
        *((stack_top - 8) as *mut u64) = entry;
        // Push the argument
        *((stack_top - 16) as *mut u64) = arg;
        // Push initial rbp (0 to mark stack bottom)
        *((stack_top - 24) as *mut u64) = 0;
    }

    ctx.rsp = rsp;
    ctx.rbx = 0;
    ctx.rbp = 0;
    ctx.r12 = 0;
    ctx.r13 = 0;
    ctx.r14 = 0;
    ctx.r15 = 0;
}
