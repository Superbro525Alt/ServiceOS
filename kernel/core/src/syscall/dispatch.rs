use core::sync::atomic::{AtomicU64, Ordering};

use spin::Once;

use super::{
    MAX_SYSCALL_SLOTS, SyscallContext, SyscallDispatcher, SyscallError, SyscallNumber,
    SyscallReturn, SyscallSnapshot, handle_abi_version, handle_audio_endpoint_info,
    handle_audio_endpoint_play_tone, handle_audio_endpoint_stop, handle_channel_create,
    handle_channel_receive, handle_channel_send, handle_debug_console_read,
    handle_debug_console_write, handle_debug_log_write, handle_display_output_info,
    handle_display_output_present, handle_handle_close, handle_handle_duplicate,
    handle_input_source_info, handle_input_source_receive, handle_memory_create,
    handle_memory_read, handle_memory_write, handle_monotonic_now, handle_packet_interface_info,
    handle_packet_interface_receive, handle_packet_interface_transmit, handle_service_spawn,
    handle_task_status, handle_thread_exit, handle_yield_current,
};

type Handler = fn(&SyscallContext) -> SyscallReturn;

pub struct DispatchTable {
    entries: [Option<Handler>; MAX_SYSCALL_SLOTS],
    dispatched: AtomicU64,
    rejected: AtomicU64,
}

impl DispatchTable {
    pub const fn new(entries: [Option<Handler>; MAX_SYSCALL_SLOTS]) -> Self {
        Self {
            entries,
            dispatched: AtomicU64::new(0),
            rejected: AtomicU64::new(0),
        }
    }

    pub fn snapshot(&self) -> SyscallSnapshot {
        SyscallSnapshot {
            dispatched: self.dispatched.load(Ordering::Relaxed),
            rejected: self.rejected.load(Ordering::Relaxed),
        }
    }
}

impl SyscallDispatcher for DispatchTable {
    fn dispatch(&self, number: SyscallNumber, context: &SyscallContext) -> SyscallReturn {
        self.dispatched.fetch_add(1, Ordering::Relaxed);

        let handler = self
            .entries
            .get(number.0 as usize)
            .and_then(|entry| entry.as_ref().copied());

        match handler {
            Some(handler) => handler(context),
            None => {
                self.rejected.fetch_add(1, Ordering::Relaxed);
                SyscallReturn::error(SyscallError::InvalidCall)
            }
        }
    }
}

static DISPATCHER: Once<DispatchTable> = Once::new();

pub fn initialize() -> &'static DispatchTable {
    DISPATCHER.call_once(|| {
        DispatchTable::new([
            Some(handle_abi_version),
            Some(handle_monotonic_now),
            Some(handle_thread_exit),
            Some(handle_yield_current),
            Some(handle_debug_log_write),
            Some(handle_channel_create),
            Some(handle_channel_send),
            Some(handle_channel_receive),
            Some(handle_handle_duplicate),
            Some(handle_handle_close),
            Some(handle_service_spawn),
            Some(handle_task_status),
            Some(handle_memory_read),
            Some(handle_debug_console_read),
            Some(handle_debug_console_write),
            Some(handle_packet_interface_info),
            Some(handle_packet_interface_receive),
            Some(handle_packet_interface_transmit),
            Some(handle_display_output_info),
            Some(handle_display_output_present),
            Some(handle_input_source_info),
            Some(handle_input_source_receive),
            Some(handle_memory_create),
            Some(handle_memory_write),
            Some(handle_audio_endpoint_info),
            Some(handle_audio_endpoint_play_tone),
            Some(handle_audio_endpoint_stop),
        ])
    })
}

pub fn dispatcher() -> Option<&'static DispatchTable> {
    DISPATCHER.get()
}
