use serviceos_abi::{
    AudioEndpointInfo as AbiAudioEndpointInfo, AudioToneRequest as AbiAudioToneRequest,
    BlockDeviceInfo as AbiBlockDeviceInfo, DisplayOutputInfo as AbiDisplayOutputInfo, Handle,
    INPUT_SOURCE_FLAG_NONBLOCK, InputEventInfo as AbiInputEventInfo,
    InputSourceInfo as AbiInputSourceInfo, PACKET_INTERFACE_FLAG_NONBLOCK,
    PacketInterfaceInfo as AbiPacketInterfaceInfo, PacketRingLayout as AbiPacketRingLayout,
};

use super::super::{
    SyscallAction, SyscallContext, SyscallError, SyscallReturn,
    resolve::{current_task, resolve_object},
    user_mut, user_ref, user_slice, user_slice_mut,
};
use crate::capability::CapabilityRights;
use crate::network::ring::{self, PageFrameStorage};
use crate::object::{DmaSafety, MemoryAccessError};

pub(crate) fn handle_block_device_info(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let object = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::READ,
    ) {
        Ok(view) => view.object,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(device) = object.block_device() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(info_out) = (unsafe { user_mut::<AbiBlockDeviceInfo>(context.arguments[1]) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    *info_out = device.info();
    SyscallReturn::success(0)
}

pub(crate) fn handle_block_device_read(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let object = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::READ,
    ) {
        Ok(view) => view.object,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(device) = object.block_device() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(length) = usize::try_from(context.arguments[3]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(buffer) = (unsafe { user_slice_mut(context.arguments[2], length) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    match device.read_blocks(context.arguments[1], buffer) {
        Ok(bytes) => SyscallReturn::success(bytes as u64),
        Err(crate::block::BlockDeviceError::InvalidOffset) => {
            SyscallReturn::error(SyscallError::InvalidArgument)
        }
        Err(crate::block::BlockDeviceError::BufferSize) => {
            SyscallReturn::error(SyscallError::BufferTooSmall)
        }
        Err(crate::block::BlockDeviceError::Busy) => SyscallReturn::error(SyscallError::Busy),
        Err(crate::block::BlockDeviceError::Unsupported) => {
            SyscallReturn::error(SyscallError::Unsupported)
        }
        Err(crate::block::BlockDeviceError::Denied) => {
            SyscallReturn::error(SyscallError::PermissionDenied)
        }
    }
}

pub(crate) fn handle_block_device_write(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let object = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::WRITE,
    ) {
        Ok(view) => view.object,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(device) = object.block_device() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(length) = usize::try_from(context.arguments[3]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(buffer) = (unsafe { user_slice(context.arguments[2], length) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    match device.write_blocks(context.arguments[1], buffer) {
        Ok(bytes) => SyscallReturn::success(bytes as u64),
        Err(crate::block::BlockDeviceError::InvalidOffset) => {
            SyscallReturn::error(SyscallError::InvalidArgument)
        }
        Err(crate::block::BlockDeviceError::BufferSize) => {
            SyscallReturn::error(SyscallError::BufferTooSmall)
        }
        Err(crate::block::BlockDeviceError::Busy) => SyscallReturn::error(SyscallError::Busy),
        Err(crate::block::BlockDeviceError::Unsupported) => {
            SyscallReturn::error(SyscallError::Unsupported)
        }
        Err(crate::block::BlockDeviceError::Denied) => {
            SyscallReturn::error(SyscallError::PermissionDenied)
        }
    }
}

pub(crate) fn handle_packet_interface_info(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let object = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::READ,
    ) {
        Ok(view) => view.object,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(interface) = object.packet_interface() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(info_out) = (unsafe { user_mut::<AbiPacketInterfaceInfo>(context.arguments[1]) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    *info_out = interface.info();
    SyscallReturn::success(0)
}

pub(crate) fn handle_packet_interface_receive(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let view = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::READ.union(CapabilityRights::WAIT),
    ) {
        Ok(view) => view,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(interface) = view.object.packet_interface() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(length) = usize::try_from(context.arguments[2]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(buffer) = (unsafe { user_slice_mut(context.arguments[1], length) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    match interface.receive(buffer) {
        Ok(received) => SyscallReturn::success(received as u64),
        Err(crate::network::PacketInterfaceError::QueueEmpty)
            if context.arguments[3] as u32 & PACKET_INTERFACE_FLAG_NONBLOCK != 0 =>
        {
            SyscallReturn::error(SyscallError::QueueEmpty)
        }
        Err(crate::network::PacketInterfaceError::QueueEmpty) => SyscallReturn::error_with_action(
            SyscallError::QueueEmpty,
            SyscallAction::BlockCurrentThreadOnPacketReceive {
                interface: view.object.id(),
            },
        ),
        Err(crate::network::PacketInterfaceError::BufferTooSmall) => {
            SyscallReturn::error(SyscallError::BufferTooSmall)
        }
        Err(crate::network::PacketInterfaceError::Busy) => SyscallReturn::error(SyscallError::Busy),
        Err(crate::network::PacketInterfaceError::Unsupported) => {
            SyscallReturn::error(SyscallError::Unsupported)
        }
    }
}

/// Negotiate a shared RX packet ring for this packet interface: create the
/// memory object (header page + one page per slot), initialize its ring
/// header, attach the kernel-side producer to the interface, and hand back
/// the consumer handle plus the wire layout. On any failure the interface
/// keeps its legacy copied-frame path untouched.
pub(crate) fn handle_packet_interface_ring_setup(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let view = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::READ,
    ) {
        Ok(view) => view,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(interface) = view.object.packet_interface() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(layout_out) = (unsafe { user_mut::<AbiPacketRingLayout>(context.arguments[1]) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Some(objects) = crate::object::model() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(task) = current_task.task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };

    let slot_count = ring::RING_DEFAULT_SLOTS;
    let total_bytes = ring::ring_total_bytes(slot_count);
    let memory_object =
        objects
            .registry()
            .create_memory_object(total_bytes, true, DmaSafety::PagePinned);
    let Some(memory) = memory_object.memory_object() else {
        return SyscallReturn::error(SyscallError::Busy);
    };
    // Materialize the real backing pages so both sides share physical
    // frames. Each slot owns exactly one whole page (network/ring.rs), so
    // the PagePinned classification is satisfiable; device_backing enforces
    // the DMA policy before any physical surface is handed out.
    let frames = match memory.device_backing() {
        Ok(frames) => frames,
        Err(MemoryAccessError::DmaPolicyViolation) => {
            return SyscallReturn::error(SyscallError::InvalidArgument);
        }
        Err(_) => return SyscallReturn::error(SyscallError::Busy),
    };

    let mut storage = PageFrameStorage { frames };
    ring::init(&mut storage, slot_count);

    if interface.has_shared_ring() {
        // One negotiation per interface; the legacy path stays available to
        // any consumer that never asks for a ring.
        return SyscallReturn::error(SyscallError::Busy);
    }
    interface.attach_shared_ring(storage, slot_count);

    let installed = match task.capability_space().install(
        memory_object,
        CapabilityRights::memory_object(),
        None,
    ) {
        Ok(handle) => handle,
        Err(error) => return SyscallReturn::error(super::common::map_capability_error(error)),
    };

    *layout_out = AbiPacketRingLayout {
        magic: ring::RING_MAGIC,
        version: ring::RING_VERSION,
        slot_count: slot_count as u32,
        slot_data_bytes: ring::RING_SLOT_DATA_BYTES as u32,
        slot_stride_bytes: crate::memory::PAGE_SIZE_BYTES as u32,
        total_bytes: total_bytes as u32,
    };
    SyscallReturn::success(installed.0 as u64)
}

/// Negotiate a shared TX packet ring (the TX mirror of the RX negotiation):
/// create and initialize a memory-object-backed image whose slots the
/// network-service fills with outbound frames, attach the kernel-side
/// consumer that drains them through the backend on doorbell, and hand back
/// the producer handle plus the wire layout. On any failure the legacy
/// copied-transmit path stays untouched.
pub(crate) fn handle_packet_interface_tx_ring_setup(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let view = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::WRITE,
    ) {
        Ok(view) => view,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(interface) = view.object.packet_interface() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(layout_out) = (unsafe { user_mut::<AbiPacketRingLayout>(context.arguments[1]) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Some(objects) = crate::object::model() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(task) = current_task.task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };

    if interface.has_shared_tx_ring() {
        // One TX negotiation per interface; the legacy copied path remains
        // available to any consumer that never asks for a ring.
        return SyscallReturn::error(SyscallError::Busy);
    }

    let slot_count = ring::RING_DEFAULT_SLOTS;
    let total_bytes = ring::ring_total_bytes(slot_count);
    let memory_object =
        objects
            .registry()
            .create_memory_object(total_bytes, true, DmaSafety::PagePinned);
    let Some(memory) = memory_object.memory_object() else {
        return SyscallReturn::error(SyscallError::Busy);
    };
    // Materialize the real backing pages so both sides share physical
    // frames. Each slot owns exactly one whole page (network/ring.rs), so
    // the PagePinned classification is satisfiable; device_backing enforces
    // the DMA policy before any physical surface is handed out.
    let frames = match memory.device_backing() {
        Ok(frames) => frames,
        Err(MemoryAccessError::DmaPolicyViolation) => {
            return SyscallReturn::error(SyscallError::InvalidArgument);
        }
        Err(_) => return SyscallReturn::error(SyscallError::Busy),
    };

    let mut storage = PageFrameStorage { frames };
    ring::init(&mut storage, slot_count);
    interface.attach_shared_tx_ring(storage, slot_count);

    let installed = match task.capability_space().install(
        memory_object,
        CapabilityRights::memory_object(),
        None,
    ) {
        Ok(handle) => handle,
        Err(error) => return SyscallReturn::error(super::common::map_capability_error(error)),
    };

    *layout_out = AbiPacketRingLayout {
        magic: ring::RING_MAGIC,
        version: ring::RING_VERSION,
        slot_count: slot_count as u32,
        slot_data_bytes: ring::RING_SLOT_DATA_BYTES as u32,
        slot_stride_bytes: crate::memory::PAGE_SIZE_BYTES as u32,
        total_bytes: total_bytes as u32,
    };
    SyscallReturn::success(installed.0 as u64)
}

/// Doorbell for the shared TX ring: drain every frame the producer has
/// published into the shared slots through the backend transmit path.
/// Returns the number of frames transmitted; frames the backend could not
/// accept stay pending for the next doorbell.
pub(crate) fn handle_packet_interface_tx_ring_flush(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let object = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::WRITE,
    ) {
        Ok(view) => view.object,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(interface) = object.packet_interface() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    SyscallReturn::success(interface.flush_transmits() as u64)
}

pub(crate) fn handle_packet_interface_transmit(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let object = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::WRITE,
    ) {
        Ok(view) => view.object,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(interface) = object.packet_interface() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(length) = usize::try_from(context.arguments[2]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(buffer) = (unsafe { user_slice(context.arguments[1], length) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    match interface.transmit(buffer) {
        Ok(()) => SyscallReturn::success(length as u64),
        Err(crate::network::PacketInterfaceError::QueueEmpty) => {
            SyscallReturn::error(SyscallError::QueueEmpty)
        }
        Err(crate::network::PacketInterfaceError::BufferTooSmall) => {
            SyscallReturn::error(SyscallError::BufferTooSmall)
        }
        Err(crate::network::PacketInterfaceError::Busy) => SyscallReturn::error(SyscallError::Busy),
        Err(crate::network::PacketInterfaceError::Unsupported) => {
            SyscallReturn::error(SyscallError::Unsupported)
        }
    }
}

pub(crate) fn handle_display_output_info(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let object = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::READ,
    ) {
        Ok(view) => view.object,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(output) = object.display_output() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(info_out) = (unsafe { user_mut::<AbiDisplayOutputInfo>(context.arguments[1]) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    *info_out = output.info();
    SyscallReturn::success(0)
}

pub(crate) fn handle_display_output_present(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let object = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::WRITE,
    ) {
        Ok(view) => view.object,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(output) = object.display_output() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(length) = usize::try_from(context.arguments[2]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(buffer) = (unsafe { user_slice(context.arguments[1], length) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    match output.present(buffer) {
        Ok(()) => SyscallReturn::success(length as u64),
        Err(crate::display::DisplayOutputError::BufferTooSmall) => {
            SyscallReturn::error(SyscallError::BufferTooSmall)
        }
        Err(crate::display::DisplayOutputError::Busy) => SyscallReturn::error(SyscallError::Busy),
        Err(crate::display::DisplayOutputError::Unsupported) => {
            SyscallReturn::error(SyscallError::Unsupported)
        }
    }
}

pub(crate) fn handle_display_output_present_damage(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let object = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::WRITE,
    ) {
        Ok(view) => view.object,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(output) = object.display_output() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(length) = usize::try_from(context.arguments[2]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(buffer) = (unsafe { user_slice(context.arguments[1], length) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    let packed_position = context.arguments[3];
    let packed_size = context.arguments[4];
    let x = packed_position as u32 as i32;
    let y = (packed_position >> 32) as u32 as i32;
    let width = packed_size as u32;
    let height = (packed_size >> 32) as u32;

    match output.present_damage(buffer, x, y, width, height) {
        Ok(()) => SyscallReturn::success(length as u64),
        Err(crate::display::DisplayOutputError::BufferTooSmall) => {
            SyscallReturn::error(SyscallError::BufferTooSmall)
        }
        Err(crate::display::DisplayOutputError::Busy) => SyscallReturn::error(SyscallError::Busy),
        Err(crate::display::DisplayOutputError::Unsupported) => {
            SyscallReturn::error(SyscallError::Unsupported)
        }
    }
}

pub(crate) fn handle_input_source_info(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let object = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::READ,
    ) {
        Ok(view) => view.object,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(source) = object.input_source() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(info_out) = (unsafe { user_mut::<AbiInputSourceInfo>(context.arguments[1]) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    *info_out = source.info();
    SyscallReturn::success(0)
}

pub(crate) fn handle_input_source_receive(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let view = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::READ.union(CapabilityRights::WAIT),
    ) {
        Ok(view) => view,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(source) = view.object.input_source() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(event_out) = (unsafe { user_mut::<AbiInputEventInfo>(context.arguments[1]) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    let receive_result = if context.arguments[2] as u32 & INPUT_SOURCE_FLAG_NONBLOCK != 0 {
        source.try_receive_with_fallback()
    } else {
        source.receive()
    };

    match receive_result {
        Ok(event) => {
            *event_out = event;
            SyscallReturn::success(0)
        }
        Err(crate::input::InputSourceError::QueueEmpty)
            if context.arguments[2] as u32 & INPUT_SOURCE_FLAG_NONBLOCK != 0 =>
        {
            SyscallReturn::error(SyscallError::QueueEmpty)
        }
        Err(crate::input::InputSourceError::QueueEmpty) => SyscallReturn::error_with_action(
            SyscallError::QueueEmpty,
            SyscallAction::BlockCurrentThreadOnInputReceive {
                source: view.object.id(),
            },
        ),
        Err(crate::input::InputSourceError::Busy) => SyscallReturn::error(SyscallError::Busy),
        Err(crate::input::InputSourceError::Unsupported) => {
            SyscallReturn::error(SyscallError::Unsupported)
        }
    }
}

pub(crate) fn handle_audio_endpoint_info(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let object = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::READ,
    ) {
        Ok(view) => view.object,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(endpoint) = object.audio_endpoint() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(info_out) = (unsafe { user_mut::<AbiAudioEndpointInfo>(context.arguments[1]) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    *info_out = endpoint.info();
    SyscallReturn::success(0)
}

pub(crate) fn handle_audio_endpoint_play_tone(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let object = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::WRITE,
    ) {
        Ok(view) => view.object,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(endpoint) = object.audio_endpoint() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(request) = (unsafe { user_ref::<AbiAudioToneRequest>(context.arguments[1]) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    match endpoint.play_tone(*request) {
        Ok(()) => SyscallReturn::success(0),
        Err(crate::audio::AudioEndpointError::Busy) => SyscallReturn::error(SyscallError::Busy),
        Err(crate::audio::AudioEndpointError::Unsupported) => {
            SyscallReturn::error(SyscallError::Unsupported)
        }
    }
}

pub(crate) fn handle_audio_endpoint_stop(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let object = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::WRITE,
    ) {
        Ok(view) => view.object,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(endpoint) = object.audio_endpoint() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    match endpoint.stop() {
        Ok(()) => SyscallReturn::success(0),
        Err(crate::audio::AudioEndpointError::Busy) => SyscallReturn::error(SyscallError::Busy),
        Err(crate::audio::AudioEndpointError::Unsupported) => {
            SyscallReturn::error(SyscallError::Unsupported)
        }
    }
}

/// Upper bound on PCM bytes accepted per call; bounds in-kernel DMA time.
const AUDIO_PCM_WRITE_MAX_BYTES: usize = 16 * 1024;

pub(crate) fn handle_audio_endpoint_pcm_write(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let object = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::WRITE,
    ) {
        Ok(view) => view.object,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(endpoint) = object.audio_endpoint() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let length = context.arguments[2] as usize;
    if length == 0 || length > AUDIO_PCM_WRITE_MAX_BYTES || length % 4 != 0 {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    }
    let Ok(bytes) = (unsafe { user_slice(context.arguments[1], length) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    match endpoint.pcm_write_s16le_stereo(bytes) {
        Ok(written) => SyscallReturn::success(written as u64),
        Err(crate::audio::AudioEndpointError::Busy) => SyscallReturn::error(SyscallError::Busy),
        Err(crate::audio::AudioEndpointError::Unsupported) => {
            SyscallReturn::error(SyscallError::Unsupported)
        }
    }
}
