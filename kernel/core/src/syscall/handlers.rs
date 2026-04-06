mod common;
mod devices;
mod ipc;
mod memory;
mod object;
mod system;
mod task;

pub(crate) use common::{
    DEBUG_CONSOLE_READER, DEBUG_CONSOLE_WRITER, DEBUG_LOG_WRITER, map_capability_error,
};
pub(crate) use devices::{
    handle_audio_endpoint_info, handle_audio_endpoint_play_tone, handle_audio_endpoint_stop,
    handle_block_device_info, handle_block_device_read, handle_block_device_write,
    handle_display_output_info, handle_display_output_present,
    handle_display_output_present_damage, handle_input_source_info, handle_input_source_receive,
    handle_packet_interface_info, handle_packet_interface_receive,
    handle_packet_interface_transmit,
};
pub(crate) use ipc::{
    handle_channel_create, handle_channel_receive, handle_channel_send, handle_handle_close,
    handle_handle_duplicate,
};
pub(crate) use memory::{
    handle_memory_create, handle_memory_info, handle_memory_map, handle_memory_map_range,
    handle_memory_read, handle_memory_write,
};
pub(crate) use object::{
    handle_event_create, handle_event_reset, handle_event_signal, handle_object_info,
    handle_object_wait,
};
pub(crate) use system::{
    handle_abi_version, handle_debug_console_read, handle_debug_console_write,
    handle_debug_log_write, handle_kernel_event_query_info, handle_kernel_event_query_record,
    handle_monotonic_now, handle_thread_exit, handle_yield_current,
};
pub(crate) use task::{handle_service_spawn, handle_task_spawn_image, handle_task_status};
