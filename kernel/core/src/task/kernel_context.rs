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

/// Initialize a kernel thread's context for the first run
///
/// This sets up the context so that when `kernel_context_switch` is called
/// to switch to this thread, it will start executing at the given entry point.
pub fn init_kernel_thread_context(ctx: &mut KernelContext, entry: u64, stack_top: u64, arg: u64) {
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
