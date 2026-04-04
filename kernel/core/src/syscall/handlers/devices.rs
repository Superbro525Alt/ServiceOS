use serviceos_abi::{
    AudioEndpointInfo as AbiAudioEndpointInfo, AudioToneRequest as AbiAudioToneRequest,
    BlockDeviceInfo as AbiBlockDeviceInfo, DisplayOutputInfo as AbiDisplayOutputInfo, Handle,
    INPUT_SOURCE_FLAG_NONBLOCK, InputEventInfo as AbiInputEventInfo,
    InputSourceInfo as AbiInputSourceInfo, PACKET_INTERFACE_FLAG_NONBLOCK,
    PacketInterfaceInfo as AbiPacketInterfaceInfo,
};

use super::super::{
    SyscallAction, SyscallContext, SyscallError, SyscallReturn,
    resolve::{current_task, resolve_object},
    user_mut, user_ref, user_slice, user_slice_mut,
};
use crate::capability::CapabilityRights;

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
