#[cfg(target_arch = "aarch64")]
mod imp {
    /// Kernel thread context saved during context switch for AArch64
    ///
    /// This structure holds the callee-saved registers that must be preserved
    /// when switching between kernel threads.
    #[repr(C)]
    #[derive(Copy, Clone, Debug)]
    pub struct KernelContext {
        /// Stack pointer
        pub sp: u64,
        /// Program counter (return address)
        pub lr: u64,
        /// Callee-saved registers
        pub x19: u64,
        pub x20: u64,
        pub x21: u64,
        pub x22: u64,
        pub x23: u64,
        pub x24: u64,
        pub x25: u64,
        pub x26: u64,
        pub x27: u64,
        pub x28: u64,
        pub x29: u64,
    }

    /// Initialize a kernel thread's context for the first run
    pub fn init_kernel_thread_context(
        ctx: &mut KernelContext,
        entry: u64,
        stack_top: u64,
        _arg: u64,
    ) {
        let sp = stack_top - 8;
        unsafe {
            *((stack_top - 8) as *mut u64) = entry;
        }

        ctx.sp = sp;
        ctx.lr = entry;
        ctx.x19 = 0;
        ctx.x20 = 0;
        ctx.x21 = 0;
        ctx.x22 = 0;
        ctx.x23 = 0;
        ctx.x24 = 0;
        ctx.x25 = 0;
        ctx.x26 = 0;
        ctx.x27 = 0;
        ctx.x28 = 0;
        ctx.x29 = 0;
    }

    /// Switch from one kernel thread context to another
    ///
    /// # Safety
    /// This function must be called with interrupts disabled and the
    /// scheduler lock held.
    pub unsafe fn kernel_context_switch(from: &mut KernelContext, to: &KernelContext) {
        unsafe {
            let sp: u64;
            let lr: u64;
            core::arch::asm!(
                "mov {}, sp",
                "mov {}, lr",
                out(reg) sp,
                out(reg) lr,
                options(nostack, nomem)
            );
            from.sp = sp;
            from.lr = lr;
            core::arch::asm!(
                "str x19, [{ctx}, #16]",
                "str x20, [{ctx}, #24]",
                "str x21, [{ctx}, #32]",
                "str x22, [{ctx}, #40]",
                "str x23, [{ctx}, #48]",
                "str x24, [{ctx}, #56]",
                "str x25, [{ctx}, #64]",
                "str x26, [{ctx}, #72]",
                "str x27, [{ctx}, #80]",
                "str x28, [{ctx}, #88]",
                "str x29, [{ctx}, #96]",
                ctx = in(reg) from as *mut KernelContext,
                options(nostack),
            );

            core::arch::asm!(
                "ldr x19, [{ctx}, #16]",
                "ldr x20, [{ctx}, #24]",
                "ldr x21, [{ctx}, #32]",
                "ldr x22, [{ctx}, #40]",
                "ldr x23, [{ctx}, #48]",
                "ldr x24, [{ctx}, #56]",
                "ldr x25, [{ctx}, #64]",
                "ldr x26, [{ctx}, #72]",
                "ldr x27, [{ctx}, #80]",
                "ldr x28, [{ctx}, #88]",
                "ldr x29, [{ctx}, #96]",
                ctx = in(reg) to as *const KernelContext,
                options(nostack),
            );
            let new_sp = to.sp;
            let new_lr = to.lr;
            core::arch::asm!(
                "mov sp, {sp}",
                "mov lr, {lr}",
                sp = in(reg) new_sp,
                lr = in(reg) new_lr,
                options(nostack, nomem)
            );
        }
    }
}

#[cfg(not(target_arch = "aarch64"))]
mod imp {
    #[derive(Copy, Clone, Debug)]
    pub struct KernelContext {
        pub sp: u64,
        pub lr: u64,
        pub x19: u64,
        pub x20: u64,
        pub x21: u64,
        pub x22: u64,
        pub x23: u64,
        pub x24: u64,
        pub x25: u64,
        pub x26: u64,
        pub x27: u64,
        pub x28: u64,
        pub x29: u64,
    }

    pub fn init_kernel_thread_context(
        _ctx: &mut KernelContext,
        _entry: u64,
        _stack_top: u64,
        _arg: u64,
    ) {
    }

    pub unsafe fn kernel_context_switch(_from: &mut KernelContext, _to: &KernelContext) {}
}

pub use imp::{KernelContext, init_kernel_thread_context, kernel_context_switch};
