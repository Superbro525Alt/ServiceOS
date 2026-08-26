use core::sync::atomic::{AtomicU64, Ordering};

use spin::Once;

use super::{
    MAX_SYSCALL_SLOTS, SyscallContext, SyscallDispatcher, SyscallError, SyscallNumber,
    SyscallReturn, SyscallSnapshot, handle_abi_version, handle_audio_endpoint_info,
    handle_audio_endpoint_pcm_write, handle_audio_endpoint_play_tone, handle_audio_endpoint_stop,
    handle_block_device_info, handle_block_device_read, handle_block_device_write,
    handle_channel_create, handle_channel_receive, handle_channel_send, handle_debug_console_read,
    handle_debug_console_write, handle_debug_log_write, handle_display_output_info,
    handle_display_output_present, handle_display_output_present_damage, handle_event_create,
    handle_event_reset, handle_event_signal, handle_fault_handler_register,
    handle_fault_handler_unregister, handle_handle_close, handle_handle_duplicate,
    handle_input_source_info, handle_input_source_receive, handle_kernel_event_query_info,
    handle_kernel_event_query_record, handle_memory_create, handle_memory_info, handle_memory_map,
    handle_memory_map_range, handle_memory_protect, handle_memory_query, handle_memory_read,
    handle_memory_unmap, handle_memory_write, handle_monotonic_now, handle_object_info,
    handle_object_wait, handle_packet_interface_info, handle_packet_interface_receive,
    handle_packet_interface_transmit, handle_packet_interface_ring_setup, handle_pipe_create,
    handle_pipe_read, handle_pipe_write,
    handle_service_spawn, handle_task_loaded_libraries, handle_task_spawn_image,
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
        let mut entries: [Option<Handler>; MAX_SYSCALL_SLOTS] = [None; MAX_SYSCALL_SLOTS];
        entries[0] = Some(handle_abi_version);
        entries[1] = Some(handle_monotonic_now);
        entries[2] = Some(handle_thread_exit);
        entries[3] = Some(handle_yield_current);
        entries[4] = Some(handle_debug_log_write);
        entries[5] = Some(handle_channel_create);
        entries[6] = Some(handle_channel_send);
        entries[7] = Some(handle_channel_receive);
        entries[8] = Some(handle_handle_duplicate);
        entries[9] = Some(handle_handle_close);
        entries[10] = Some(handle_service_spawn);
        entries[11] = Some(handle_task_status);
        entries[12] = Some(handle_memory_read);
        entries[13] = Some(handle_debug_console_read);
        entries[14] = Some(handle_debug_console_write);
        entries[15] = Some(handle_packet_interface_info);
        entries[16] = Some(handle_packet_interface_receive);
        entries[17] = Some(handle_packet_interface_transmit);
        entries[18] = Some(handle_display_output_info);
        entries[19] = Some(handle_display_output_present);
        entries[20] = Some(handle_input_source_info);
        entries[21] = Some(handle_input_source_receive);
        entries[22] = Some(handle_memory_create);
        entries[23] = Some(handle_memory_write);
        entries[24] = Some(handle_audio_endpoint_info);
        entries[25] = Some(handle_audio_endpoint_play_tone);
        entries[26] = Some(handle_audio_endpoint_stop);
        entries[27] = Some(handle_memory_map);
        entries[28] = Some(handle_task_spawn_image);
        entries[29] = Some(handle_block_device_info);
        entries[30] = Some(handle_block_device_read);
        entries[31] = Some(handle_block_device_write);
        entries[32] = Some(handle_memory_info);
        entries[33] = Some(handle_memory_map_range);
        entries[34] = Some(handle_event_create);
        entries[35] = Some(handle_event_signal);
        entries[36] = Some(handle_event_reset);
        entries[37] = Some(handle_object_info);
        entries[38] = Some(handle_object_wait);
        entries[39] = Some(handle_kernel_event_query_info);
        entries[40] = Some(handle_kernel_event_query_record);
        entries[41] = Some(handle_display_output_present_damage);
        entries[42] = Some(handle_memory_unmap);
        entries[43] = Some(handle_memory_protect);
        entries[44] = Some(handle_memory_query);
        entries[45] = Some(handle_fault_handler_register);
        entries[46] = Some(handle_fault_handler_unregister);
        entries[47] = Some(handle_task_loaded_libraries);
        entries[48] = Some(handle_audio_endpoint_pcm_write);
        entries[49] = Some(handle_pipe_create);
        entries[50] = Some(handle_pipe_read);
        entries[51] = Some(handle_pipe_write);
        entries[52] = Some(handle_packet_interface_ring_setup);
        DispatchTable::new(entries)
    })
}

pub fn dispatcher() -> Option<&'static DispatchTable> {
    DISPATCHER.get()
}
