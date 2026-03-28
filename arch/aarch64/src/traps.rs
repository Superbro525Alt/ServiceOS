#[cfg(target_arch = "aarch64")]
mod imp {
    use core::arch::global_asm;

    use serviceos_kernel_core::{
        interrupts::{self, ExceptionDetail, ExceptionVector, FaultDisposition, TrapFrameView},
        syscall::{SyscallContext, SyscallNumber},
        task::system,
        user,
    };

    use crate::user::SavedUserContext;

    const ESR_EC_SHIFT: u64 = 26;
    const ESR_EC_MASK: u64 = 0x3f;
    const ESR_EC_SVC64: u64 = 0x15;
    const ESR_EC_INSTRUCTION_ABORT_LOWER_EL: u64 = 0x20;
    const ESR_EC_DATA_ABORT_LOWER_EL: u64 = 0x24;
    const ESR_EC_BRK64_LOWER_EL: u64 = 0x3c;

    global_asm!(
        r#"
.global serviceos_aarch64_vector_table
serviceos_aarch64_vector_table:
    b serviceos_aarch64_fatal_vector
    b serviceos_aarch64_fatal_vector
    b serviceos_aarch64_fatal_vector
    b serviceos_aarch64_fatal_vector
    .balign 0x80
    b serviceos_aarch64_fatal_vector
    b serviceos_aarch64_fatal_vector
    b serviceos_aarch64_fatal_vector
    b serviceos_aarch64_fatal_vector
    .balign 0x80
    b serviceos_aarch64_lower_el_sync
    b serviceos_aarch64_fatal_vector
    b serviceos_aarch64_fatal_vector
    b serviceos_aarch64_fatal_vector
    .balign 0x80
    b serviceos_aarch64_fatal_vector
    b serviceos_aarch64_fatal_vector
    b serviceos_aarch64_fatal_vector
    b serviceos_aarch64_fatal_vector

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
    extern "C" fn serviceos_aarch64_handle_user_sync(context: &mut SavedUserContext) -> u64 {
        let ec = (context.esr_el1 >> ESR_EC_SHIFT) & ESR_EC_MASK;
        match ec {
            ESR_EC_SVC64 => handle_syscall(context),
            ESR_EC_BRK64_LOWER_EL => {
                context.elr_el1 = context.elr_el1.saturating_add(4);
                0
            }
            ESR_EC_INSTRUCTION_ABORT_LOWER_EL | ESR_EC_DATA_ABORT_LOWER_EL => {
                terminate_current_user_context(context);
                1
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

    fn handle_syscall(context: &mut SavedUserContext) -> u64 {
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
        match result.action {
            serviceos_kernel_core::syscall::SyscallAction::ReturnToCaller => 0,
            serviceos_kernel_core::syscall::SyscallAction::YieldCurrentThread => {
                if let Some(tasks) = system() {
                    let _ = tasks.scheduler().yield_current();
                }
                1
            }
            serviceos_kernel_core::syscall::SyscallAction::BlockCurrentThreadOnReceive {
                endpoint,
            } => {
                if let Some(tasks) = system() {
                    let _ = tasks.scheduler().block_current_on_receive(endpoint);
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
            serviceos_kernel_core::syscall::SyscallAction::ExitCurrentThread { status } => {
                user::mark_current_thread_exited(status);
                if let Some(tasks) = system() {
                    let _ = tasks.scheduler().terminate_current();
                }
                1
            }
        }
    }

    fn terminate_current_user_context(context: &SavedUserContext) {
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
        user::mark_current_thread_exited(0xffff_ffff_ffff_ff01);
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
