use serviceos_kernel_core::{
    interrupts,
    syscall::{SyscallAction, SyscallContext, SyscallNumber},
    task,
};

use crate::user::SavedUserContext;

#[unsafe(no_mangle)]
extern "C" fn serviceos_x86_64_handle_syscall(frame: &mut SavedUserContext) -> u64 {
    let context = SyscallContext {
        instruction_pointer: frame.instruction_pointer,
        stack_pointer: frame.user_stack_pointer,
        flags: frame.cpu_flags,
        arguments: [
            frame.rdi, frame.rsi, frame.rdx, frame.r10, frame.r8, frame.r9,
        ],
    };
    let result = interrupts::dispatch_syscall(SyscallNumber(frame.rax as u32), &context);

    frame.rax = result.value;
    frame.rdx = result.abi_error_code();
    match result.action {
        SyscallAction::ReturnToCaller => 0,
        SyscallAction::YieldCurrentThread => {
            if let Some(tasks) = task::system() {
                if let Some(thread_id) = tasks.scheduler().current_thread() {
                    crate::user::save_thread_context(thread_id, frame);
                }
                let _ = tasks.scheduler().yield_current();
            }
            1
        }
        SyscallAction::BlockCurrentThreadOnReceive { endpoint } => {
            if let Some(tasks) = task::system() {
                if let Some(thread_id) = tasks.scheduler().current_thread() {
                    crate::user::save_thread_context(thread_id, frame);
                }
                let _ = tasks.scheduler().block_current_on_receive(endpoint);
            }
            1
        }
        SyscallAction::BlockCurrentThreadOnPacketReceive { interface } => {
            if let Some(tasks) = task::system() {
                if let Some(thread_id) = tasks.scheduler().current_thread() {
                    crate::user::save_thread_context(thread_id, frame);
                }
                let _ = tasks.scheduler().block_current_on_packet_receive(interface);
            }
            1
        }
        SyscallAction::BlockCurrentThreadOnInputReceive { source } => {
            if let Some(tasks) = task::system() {
                if let Some(thread_id) = tasks.scheduler().current_thread() {
                    crate::user::save_thread_context(thread_id, frame);
                }
                let _ = tasks.scheduler().block_current_on_input_receive(source);
            }
            1
        }
        SyscallAction::ExitCurrentThread { status } => {
            serviceos_kernel_core::user::mark_current_thread_exited(status);
            if let Some(tasks) = task::system() {
                let _ = tasks.scheduler().terminate_current();
            }
            1
        }
    }
}
