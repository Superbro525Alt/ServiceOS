#[cfg(target_arch = "aarch64")]
mod imp {
    use core::arch::global_asm;

    use serviceos_kernel_core::{
        interrupts::{self, ExceptionDetail, ExceptionVector, FaultDisposition, TrapFrameView},
        syscall::{SyscallContext, SyscallNumber},
        task::system,
        user,
    };

    use crate::gic;
    use crate::timer;
    use crate::user::SavedUserContext;

    const ESR_EC_SHIFT: u64 = 26;
    const ESR_EC_MASK: u64 = 0x3f;
    const ESR_EC_SVC64: u64 = 0x15;
    const ESR_EC_INSTRUCTION_ABORT_LOWER_EL: u64 = 0x20;
    const ESR_EC_DATA_ABORT_LOWER_EL: u64 = 0x24;
    const ESR_EC_BRK64_LOWER_EL: u64 = 0x3c;

    fn uart_trace_bytes(bytes: &[u8]) {
        const UART_DATA: *mut u8 = 0x0900_0000 as *mut u8;
        for &byte in bytes {
            unsafe { core::ptr::write_volatile(UART_DATA, byte) };
        }
    }

    fn uart_hex(value: u64) -> [u8; 18] {
        let mut out = [b'0'; 18];
        out[0] = b'0';
        out[1] = b'x';
        for nibble in 0..16 {
            let shift = 60 - nibble * 4;
            let digit = ((value >> shift) & 0xf) as u8;
            out[2 + nibble] = if digit < 10 {
                b'0' + digit
            } else {
                b'a' + digit - 10
            };
        }
        out
    }

    fn trace_sync(tag: u8, a: u64, b: u64, c: u64) {
        let mut buf = [b' '; 64];
        buf[0] = tag;
        let mut len = 1;
        for value in [a, b, c] {
            buf[len..len + 18].copy_from_slice(&uart_hex(value));
            len += 18;
        }
        buf[len] = b'\r';
        buf[len + 1] = b'\n';
        uart_trace_bytes(&buf[..len + 2]);
    }

    global_asm!(
        r#"
.global serviceos_aarch64_vector_table
.macro VEC_SLOT target
    b \target
    .space 0x7c
.endm
.balign 0x800
serviceos_aarch64_vector_table:
    VEC_SLOT serviceos_aarch64_fatal_vector
    VEC_SLOT serviceos_aarch64_fatal_vector
    VEC_SLOT serviceos_aarch64_fatal_vector
    VEC_SLOT serviceos_aarch64_fatal_vector

    VEC_SLOT serviceos_aarch64_fatal_vector
    VEC_SLOT serviceos_aarch64_current_el_irq
    VEC_SLOT serviceos_aarch64_fatal_vector
    VEC_SLOT serviceos_aarch64_fatal_vector

    VEC_SLOT serviceos_aarch64_lower_el_sync
    VEC_SLOT serviceos_aarch64_lower_el_irq
    VEC_SLOT serviceos_aarch64_fatal_vector
    VEC_SLOT serviceos_aarch64_fatal_vector

    VEC_SLOT serviceos_aarch64_fatal_vector
    VEC_SLOT serviceos_aarch64_fatal_vector
    VEC_SLOT serviceos_aarch64_fatal_vector
    VEC_SLOT serviceos_aarch64_fatal_vector

.global serviceos_aarch64_current_el_irq
serviceos_aarch64_current_el_irq:
    b serviceos_aarch64_lower_el_irq

.global serviceos_aarch64_lower_el_irq
serviceos_aarch64_lower_el_irq:
    stp x0, x1, [sp, #-16]!
    stp x2, x3, [sp, #-16]!
    stp x4, x5, [sp, #-16]!
    stp x6, x7, [sp, #-16]!
    stp x8, x9, [sp, #-16]!
    stp x10, x11, [sp, #-16]!
    stp x12, x13, [sp, #-16]!
    stp x14, x15, [sp, #-16]!
    stp x16, x17, [sp, #-16]!
    stp x18, x19, [sp, #-16]!
    stp x20, x21, [sp, #-16]!
    stp x22, x23, [sp, #-16]!
    stp x24, x25, [sp, #-16]!
    stp x26, x27, [sp, #-16]!
    stp x28, x29, [sp, #-16]!
    stp x30, xzr, [sp, #-16]!
    mov x0, sp
    bl serviceos_aarch64_handle_irq
    cbz x0, 5f
    // Preempted: abandon this frame and return into run_thread's kernel
    // continuation via the shared kernel_return_sp handoff, exactly like
    // the sync stub's scheduler-suspension exit. The pushed registers are
    // dead; the handler already snapshotted the interrupted user context.
    adrp x10, serviceos_aarch64_kernel_return_sp
    add x10, x10, :lo12:serviceos_aarch64_kernel_return_sp
    ldr x9, [x10]
    mov sp, x9
    ldp x29, x30, [sp], #16
    ldp x27, x28, [sp], #16
    ldp x25, x26, [sp], #16
    ldp x23, x24, [sp], #16
    ldp x21, x22, [sp], #16
    ldp x19, x20, [sp], #16
    ret
5:
    ldp x30, xzr, [sp], #16
    ldp x28, x29, [sp], #16
    ldp x26, x27, [sp], #16
    ldp x24, x25, [sp], #16
    ldp x22, x23, [sp], #16
    ldp x20, x21, [sp], #16
    ldp x18, x19, [sp], #16
    ldp x16, x17, [sp], #16
    ldp x14, x15, [sp], #16
    ldp x12, x13, [sp], #16
    ldp x10, x11, [sp], #16
    ldp x8, x9, [sp], #16
    ldp x6, x7, [sp], #16
    ldp x4, x5, [sp], #16
    ldp x2, x3, [sp], #16
    ldp x0, x1, [sp], #16
    eret

.global serviceos_aarch64_fatal_vector
serviceos_aarch64_fatal_vector:
1:
    wfe
    b 1b
"#
    );

    unsafe extern "C" {
        static serviceos_aarch64_vector_table: u8;
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct TrapBringupStatus {
        pub vector_table: bool,
    }

    pub const fn bringup_status() -> TrapBringupStatus {
        TrapBringupStatus { vector_table: true }
    }

    pub fn initialize() {
        unsafe {
            core::arch::asm!(
                "msr vbar_el1, {base}",
                "isb",
                base = in(reg) (&serviceos_aarch64_vector_table as *const u8 as u64),
                options(nostack)
            );
        }
    }

    #[unsafe(no_mangle)]
    extern "C" fn serviceos_aarch64_handle_irq(saved_regs: *const u64) -> u64 {
        let Some(irq) = gic::acknowledge() else {
            return 0;
        };
        let timer_tick = irq.intid == gic::timer_ppi_intid();
        if timer_tick {
            timer::rearm_periodic_tick();
            interrupts::note_timer_interrupt(interrupts::InterruptVector(irq.intid));
        } else {
            interrupts::note_external_interrupt(interrupts::InterruptVector(irq.intid));
            // Ack the owning device before the EOI so a held level line
            // deasserts instead of storming; hook is lock-free by contract.
            interrupts::dispatch_external_irq(irq.intid);
        }
        gic::end_of_interrupt(irq);

        // IRQ-return preemption, mirroring
        // arch/x86_64/src/interrupts/irq.rs::serviceos_x86_64_handle_timer_irq:
        // only the timer tick preempts, and only an interrupted user-mode
        // frame whose scheduler slice has expired. Returns 1 to make the asm
        // stub abandon this frame for run_thread's kernel continuation (the
        // thread later resumes from the snapshot saved below), or 0 to eret
        // back into the interrupted context.
        if !timer_tick {
            return 0;
        }
        let regs = unsafe { &*(saved_regs as *const [u64; 32]) };
        let (sp_el0, elr_el1, spsr_el1, esr_el1, far_el1): (u64, u64, u64, u64, u64);
        unsafe {
            core::arch::asm!(
                "mrs {sp}, sp_el0",
                "mrs {elr}, elr_el1",
                "mrs {spsr}, spsr_el1",
                "mrs {esr}, esr_el1",
                "mrs {far}, far_el1",
                sp = out(reg) sp_el0,
                elr = out(reg) elr_el1,
                spsr = out(reg) spsr_el1,
                esr = out(reg) esr_el1,
                far = out(reg) far_el1,
                options(nomem, nostack)
            );
        }
        // SPSR.M == 0b0000 (EL0t) identifies an interrupted user context.
        // EL1 execution always runs with IRQs masked (no daifclr outside the
        // executor idle window), so a timer frame can only originate in user
        // mode or the idle window; the M check rejects both the EL1 window
        // and the idle window (stale user SPSR while no thread runs, which
        // the current_thread() check below also rejects).
        if spsr_el1 & 0xf != 0 {
            return 0;
        }
        // Frame sanity: a healthy interrupted user frame lives inside the
        // declared user-stack window (see crate::user::USER_STACK_TOP — every
        // image's stack is 1 MiB below that top). The kernel image and its
        // boot stack are identity-mapped far below the window, so no user
        // stack can alias kernel stack memory; a sp outside the window is
        // not a healthy user context (stale or corrupted) and keeps the
        // cooperative schedule instead of snapshotting garbage. This
        // narrows the 20bbc32 guard, which compared sp against the IRQ
        // frame base: because user VAs (0x7fff_…) always sit above the
        // identity-mapped kernel frame base (0x414B_…), that check rejected
        // every user thread — not just root — and silently disabled IRQ
        // preemption entirely.
        if !(sp_el0 >= crate::user::USER_STACK_WINDOW_BOTTOM
            && sp_el0 < crate::user::USER_STACK_TOP)
        {
            return 0;
        }
        // The IRQ stub pushes register pairs from (x0, x1) up to (x30, xzr),
        // so the frame at saved_regs is in REVERSE register order: slot 0
        // holds x30, slot 30 holds x0, and slot 31 holds x1 (x30's pair
        // partner is xzr and is discarded). Map register i onto its slot:
        // even i -> 30 - i, odd i -> 32 - i. The 20bbc32 snapshot mapped
        // regs[i] -> x[i] identity-style, which swapped x0/x30 (a preempted
        // thread resumed with x30 = the live x0 — e.g. a spawn result — and
        // took a PC-alignment fault jumping to it).
        fn saved_reg(regs: &[u64; 32], i: usize) -> u64 {
            regs[30 - i + 2 * (i & 1)]
        }
        let context = SavedUserContext {
            x0: saved_reg(regs, 0),
            x1: saved_reg(regs, 1),
            x2: saved_reg(regs, 2),
            x3: saved_reg(regs, 3),
            x4: saved_reg(regs, 4),
            x5: saved_reg(regs, 5),
            x6: saved_reg(regs, 6),
            x7: saved_reg(regs, 7),
            x8: saved_reg(regs, 8),
            x9: saved_reg(regs, 9),
            x10: saved_reg(regs, 10),
            x11: saved_reg(regs, 11),
            x12: saved_reg(regs, 12),
            x13: saved_reg(regs, 13),
            x14: saved_reg(regs, 14),
            x15: saved_reg(regs, 15),
            x16: saved_reg(regs, 16),
            x17: saved_reg(regs, 17),
            x18: saved_reg(regs, 18),
            x19: saved_reg(regs, 19),
            x20: saved_reg(regs, 20),
            x21: saved_reg(regs, 21),
            x22: saved_reg(regs, 22),
            x23: saved_reg(regs, 23),
            x24: saved_reg(regs, 24),
            x25: saved_reg(regs, 25),
            x26: saved_reg(regs, 26),
            x27: saved_reg(regs, 27),
            x28: saved_reg(regs, 28),
            x29: saved_reg(regs, 29),
            x30: saved_reg(regs, 30),
            sp_el0,
            elr_el1,
            spsr_el1,
            esr_el1,
            far_el1,
            resume_publish: 0,
        };
        let Some(tasks) = system() else {
            return 0;
        };
        let scheduler = tasks.scheduler();
        if !scheduler.preemption_pending() {
            return 0;
        }
        // The context fields already mirror the interrupted frame; snapshot
        // the full user context before the frame is abandoned so the thread
        // resumes from this exact state when it is scheduled again via
        // run_thread().
        if let Some(thread_id) = scheduler.current_thread() {
            crate::user::save_thread_context(thread_id, &context);
        }
        // Requeue the current thread and select its successor now so the
        // executor loop picks up the next thread as soon as the stub's
        // abandon path returns. preempt_current_if_needed() checks and
        // clears preemption_pending under the scheduler lock; consuming the
        // flag separately first would make it early-return and leave the
        // preempted thread in place forever.
        let _ = scheduler.preempt_current_if_needed();
        1
    }

    #[unsafe(no_mangle)]
    extern "C" fn serviceos_aarch64_handle_user_sync(context: &mut SavedUserContext) -> u64 {
        let ec = (context.esr_el1 >> ESR_EC_SHIFT) & ESR_EC_MASK;
        trace_sync(b'e', ec, context.far_el1, context.elr_el1);
        match ec {
            ESR_EC_SVC64 => handle_syscall(context),
            ESR_EC_BRK64_LOWER_EL => {
                context.elr_el1 = context.elr_el1.saturating_add(4);
                0
            }
            ESR_EC_INSTRUCTION_ABORT_LOWER_EL | ESR_EC_DATA_ABORT_LOWER_EL => {
                handle_user_fault(context)
            }
            _ => {
                let report = interrupts::handle_exception(
                    ExceptionDetail::Unknown {
                        vector: ExceptionVector(ec as u8),
                        error_code: Some(context.esr_el1),
                    },
                    TrapFrameView {
                        instruction_pointer: context.elr_el1,
                        stack_pointer: context.sp_el0,
                        flags: context.spsr_el1,
                        code_segment: 0b11,
                    },
                );
                if matches!(report.disposition, FaultDisposition::TerminateTask) {
                    terminate_current_user_context(context);
                    1
                } else {
                    0
                }
            }
        }
    }

    fn handle_user_fault(context: &mut SavedUserContext) -> u64 {
        let fault_type =
            serviceos_kernel_core::fault::fault_type_for_exception(&ExceptionDetail::PageFault {
                fault_address: context.far_el1,
                error_code: context.esr_el1,
            });
        if let Some(_handler) = serviceos_kernel_core::fault::lookup_fault_handler(&fault_type) {
            let endpoint = _handler.endpoint;
            if let Some(tasks) = system() {
                tasks.notify_object_ready(endpoint);
            }
            0
        } else {
            terminate_current_user_context(context);
            1
        }
    }

    fn handle_syscall(context: &mut SavedUserContext) -> u64 {
        trace_sync(b'n', context.x8, context.x0, context.elr_el1);
        context.elr_el1 = context.elr_el1.saturating_add(4);
        let syscall_context = SyscallContext {
            instruction_pointer: context.elr_el1,
            stack_pointer: context.sp_el0,
            flags: context.spsr_el1,
            arguments: [
                context.x0, context.x1, context.x2, context.x3, context.x4, context.x5,
            ],
        };
        let result =
            interrupts::dispatch_syscall(SyscallNumber(context.x8 as u32), &syscall_context);
        context.x0 = result.value;
        context.x1 = result.abi_error_code();
        // Deliver the syscall result through the task's raw_syscall result
        // slot at [sp_el0-16, sp_el0-8], below the suspended sp. Memory
        // delivery keeps the return path on the same EL1->EL0 store/load
        // channel as channel payloads, which stays coherent across the eret
        // boundary where register-only returns were observed to revert to
        // their pre-svc values.
        unsafe {
            let result_slot = (context.sp_el0 - 16) as *mut u64;
            core::ptr::write_volatile(result_slot, result.value);
            core::ptr::write_volatile(result_slot.add(1), result.abi_error_code());
        }
        match result.action {
            serviceos_kernel_core::syscall::SyscallAction::ReturnToCaller => 0,
            serviceos_kernel_core::syscall::SyscallAction::YieldCurrentThread => {
                trace_sync(b'Y', context.x8, 0, 0);
                if let Some(tasks) = system() {
                    let _ = tasks.scheduler().yield_current();
                }
                1
            }
            serviceos_kernel_core::syscall::SyscallAction::BlockCurrentThreadOnReceive {
                endpoint,
                deadline_ticks,
            } => {
                if let Some(tasks) = system() {
                    let _ = tasks
                        .scheduler()
                        .block_current_on_receive_until(endpoint, deadline_ticks);
                }
                1
            }
            serviceos_kernel_core::syscall::SyscallAction::BlockCurrentThreadOnPacketReceive {
                interface,
            } => {
                if let Some(tasks) = system() {
                    let _ = tasks.scheduler().block_current_on_packet_receive(interface);
                }
                1
            }
            serviceos_kernel_core::syscall::SyscallAction::BlockCurrentThreadOnInputReceive {
                source,
            } => {
                if let Some(tasks) = system() {
                    let _ = tasks.scheduler().block_current_on_input_receive(source);
                }
                1
            }
            serviceos_kernel_core::syscall::SyscallAction::BlockCurrentThreadOnObject {
                object,
            } => {
                if let Some(tasks) = system() {
                    let _ = tasks.scheduler().block_current_on_object(object);
                }
                1
            }
            serviceos_kernel_core::syscall::SyscallAction::ExitCurrentThread { status } => {
                trace_sync(b'x', status, 0, 0);
                user::mark_current_thread_exited(status);
                if let Some(tasks) = system() {
                    let _ = tasks.scheduler().terminate_current();
                }
                1
            }
        }
    }

    fn terminate_current_user_context(context: &SavedUserContext) {
        trace_sync(b'T', context.esr_el1, context.far_el1, context.elr_el1);
        trace_sync(b'S', context.sp_el0, context.x30, context.spsr_el1);
        let _ = interrupts::handle_exception(
            ExceptionDetail::PageFault {
                fault_address: context.far_el1,
                error_code: context.esr_el1,
            },
            TrapFrameView {
                instruction_pointer: context.elr_el1,
                stack_pointer: context.sp_el0,
                flags: context.spsr_el1,
                code_segment: 0b11,
            },
        );
        user::mark_current_thread_faulted(0xffff_ffff_ffff_ff01);
        if let Some(tasks) = system() {
            let _ = tasks.scheduler().terminate_current();
        }
    }
}

#[cfg(not(target_arch = "aarch64"))]
mod imp {
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct TrapBringupStatus {
        pub vector_table: bool,
    }

    pub const fn bringup_status() -> TrapBringupStatus {
        TrapBringupStatus {
            vector_table: false,
        }
    }

    pub fn initialize() {}
}

pub use imp::{TrapBringupStatus, bringup_status, initialize};
