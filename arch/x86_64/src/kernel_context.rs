use serviceos_kernel_core::task::KernelContext;

#[cfg(target_arch = "x86_64")]
mod imp {
    use super::KernelContext;

    use core::arch::global_asm;

    global_asm!(
        r#"
.global serviceos_x86_64_kthread_entry
# First entry trampoline: the seeded stack places this stub address where the
# switch-in `ret` lands, followed by the argument. Popping the argument into
# rdi and returning again enters the thread entry with SysV-correct stack
# alignment (rsp % 16 == 8 at the first instruction).
serviceos_x86_64_kthread_entry:
    pop rcx
    ret
"#
    );

    unsafe extern "C" {
        fn serviceos_x86_64_kthread_entry();
    }

    /// Initialize a kernel thread context for its first run.
    ///
    /// Seeds the stack (growing downward from `stack_top`, which must be
    /// 16-aligned) so that [`kernel_context_switch`] into this context pops
    /// eight zeroed callee-saved registers, returns into the entry stub, and
    /// the stub invokes `entry(arg)` (Microsoft x64 ABI: the UEFI target's
    /// "C" convention — argument arrives in RCX).
    ///
    /// ```text
    /// stack_top - 96  ctx.rsp  [r15][r14][r13][r12][rsi][rdi][rbp][rbx] (zeroed)
    /// stack_top - 32           [resume address = entry stub]
    /// stack_top - 24           [argument]
    /// stack_top - 16           [entry function]
    /// ```
    pub fn init_kernel_thread_context(
        ctx: &mut KernelContext,
        entry: extern "C" fn(u64) -> !,
        stack_top: u64,
        arg: u64,
    ) {
        debug_assert!(stack_top % 16 == 0);

        let seed = stack_top - 96;
        unsafe {
            core::ptr::write_bytes(seed as *mut u64, 0, 8);
            core::ptr::write_volatile(
                (seed + 64) as *mut u64,
                serviceos_x86_64_kthread_entry as *const () as u64,
            );
            core::ptr::write_volatile((seed + 72) as *mut u64, arg);
            core::ptr::write_volatile((seed + 80) as *mut u64, entry as *const () as u64);
        }

        ctx.rsp = seed;
        ctx.rbx = 0;
        ctx.rbp = 0;
        ctx.r12 = 0;
        ctx.r13 = 0;
        ctx.r14 = 0;
        ctx.r15 = 0;
    }

    /// Switch from one kernel thread context to another.
    ///
    /// Saves the caller's callee-saved registers plus RSP/RIP in `from`,
    /// then restores `to`'s register state and resumes at its saved RIP via
    /// `ret`. Must be called with interrupts disabled.
    ///
    /// # Safety
    /// Both contexts must have been initialized by
    /// [`init_kernel_thread_context`] (or previously saved by this
    /// function), their stacks must still be alive, and no other CPU may be
    /// switching on the same contexts concurrently.
    ///
    /// Must be a naked function: the save/resume protocol stores and
    /// restores the true call-site return address, so the compiler must not
    /// add any prologue or epilogue of its own (an alignment `push` shifts
    /// the saved chain by one slot and makes the resuming `ret` consume
    /// garbage — verified against exactly that failure mode). The ABI is
    /// Microsoft x64 (the UEFI target's "C" convention): `from` arrives in
    /// RCX, `to` in RDX, and RBX/RBP/RDI/RSI/R12-R15 are the callee-saved
    /// set that must survive the switch.
    #[unsafe(naked)]
    pub unsafe extern "C" fn kernel_context_switch(
        from: &mut KernelContext,
        to: &KernelContext,
    ) {
        core::arch::naked_asm!(
            "push rbx",
            "push rbp",
            "push rdi",
            "push rsi",
            "push r12",
            "push r13",
            "push r14",
            "push r15",
            "mov [rcx], rsp",
            "mov rsp, [rdx]",
            "pop r15",
            "pop r14",
            "pop r13",
            "pop r12",
            "pop rsi",
            "pop rdi",
            "pop rbp",
            "pop rbx",
            "ret",
        );
    }
}

#[cfg(not(target_arch = "x86_64"))]
mod imp {
    use super::KernelContext;

    pub fn init_kernel_thread_context(
        _ctx: &mut KernelContext,
        _entry: extern "C" fn(u64) -> !,
        _stack_top: u64,
        _arg: u64,
    ) {
    }

    pub unsafe fn kernel_context_switch(_from: &mut KernelContext, _to: &KernelContext) {}
}

pub use imp::{init_kernel_thread_context, kernel_context_switch};
