mod access;
mod dispatch;
mod handlers;
mod resolve;
mod types;

pub use access::{user_mut, user_ref, user_slice, user_slice_mut};
pub use dispatch::{DispatchTable, dispatcher, initialize};
pub use types::{
    MAX_SYSCALL_SLOTS, SYSCALL_ABI_VERSION, SyscallAction, SyscallContext, SyscallDispatcher,
    SyscallError, SyscallKind, SyscallNumber, SyscallReturn, SyscallSnapshot,
};

use handlers::{
    DEBUG_CONSOLE_READER, DEBUG_CONSOLE_WRITER, DEBUG_LOG_WRITER, handle_abi_version,
    handle_channel_create, handle_channel_receive, handle_channel_send, handle_debug_console_read,
    handle_debug_console_write, handle_debug_log_write, handle_display_output_info,
    handle_display_output_present, handle_handle_close, handle_handle_duplicate,
    handle_input_source_info, handle_input_source_receive, handle_memory_create,
    handle_memory_read, handle_memory_write, handle_monotonic_now, handle_packet_interface_info,
    handle_packet_interface_receive, handle_packet_interface_transmit, handle_service_spawn,
    handle_task_status, handle_thread_exit, handle_yield_current,
};

pub fn register_debug_log_writer(writer: fn(&[u8])) {
    let _ = DEBUG_LOG_WRITER.call_once(|| writer);
}

pub fn register_debug_console_reader(reader: fn() -> Option<u8>) {
    let _ = DEBUG_CONSOLE_READER.call_once(|| reader);
}

pub fn register_debug_console_writer(writer: fn(&[u8])) {
    let _ = DEBUG_CONSOLE_WRITER.call_once(|| writer);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_context() -> SyscallContext {
        SyscallContext {
            instruction_pointer: 0,
            stack_pointer: 0,
            flags: 0,
            arguments: [0; 6],
        }
    }

    #[test]
    fn unknown_syscall_is_rejected_and_counted() {
        let mut entries: [Option<fn(&SyscallContext) -> SyscallReturn>; MAX_SYSCALL_SLOTS] =
            [None; MAX_SYSCALL_SLOTS];
        entries[0] = Some(handle_abi_version);
        entries[2] = Some(handle_thread_exit);
        let table = DispatchTable::new(entries);

        let result = table.dispatch(SyscallNumber(1), &empty_context());
        assert_eq!(result, SyscallReturn::error(SyscallError::InvalidCall));
        assert_eq!(
            table.snapshot(),
            SyscallSnapshot {
                dispatched: 1,
                rejected: 1,
            }
        );
    }

    #[test]
    fn abi_version_syscall_returns_stable_value() {
        let mut entries: [Option<fn(&SyscallContext) -> SyscallReturn>; MAX_SYSCALL_SLOTS] =
            [None; MAX_SYSCALL_SLOTS];
        entries[0] = Some(handle_abi_version);
        let table = DispatchTable::new(entries);

        let result = table.dispatch(
            SyscallNumber(SyscallKind::AbiVersion as u32),
            &empty_context(),
        );
        assert_eq!(result, SyscallReturn::success(SYSCALL_ABI_VERSION));
    }
}
