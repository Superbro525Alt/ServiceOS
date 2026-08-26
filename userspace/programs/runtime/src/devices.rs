use crate::{
    AudioEndpointInfo, AudioToneRequest, BlockDeviceBackend, BlockDeviceInfo, DisplayOutputBackend,
    DisplayOutputInfo, DisplayOutputState, DisplayPixelFormat, Handle, INPUT_SOURCE_FLAG_NONBLOCK,
    InputEventInfo, InputSourceBackend, InputSourceInfo, PACKET_INTERFACE_FLAG_NONBLOCK,
    PacketInterfaceBackend, PacketInterfaceInfo, PacketInterfaceLinkState, PacketRingLayout,
    Result, SyscallNumber, syscall1, syscall2, syscall3, syscall4, syscall5,
};

pub fn block_device_info(handle: Handle) -> Result<BlockDeviceInfo> {
    let mut info = BlockDeviceInfo {
        backend: BlockDeviceBackend::Unknown as u32,
        writable: 0,
        block_size: 0,
        reserved: 0,
        block_count: 0,
        read_ops: 0,
        write_ops: 0,
    };
    syscall2(
        SyscallNumber::BlockDeviceInfo,
        handle as u64,
        &mut info as *mut BlockDeviceInfo as u64,
    )?;
    Ok(info)
}

pub fn block_device_read(handle: Handle, start_block: u64, buffer: &mut [u8]) -> Result<usize> {
    syscall4(
        SyscallNumber::BlockDeviceRead,
        handle as u64,
        start_block,
        buffer.as_mut_ptr() as u64,
        buffer.len() as u64,
    )
    .map(|value| value as usize)
}

pub fn block_device_write(handle: Handle, start_block: u64, buffer: &[u8]) -> Result<usize> {
    syscall4(
        SyscallNumber::BlockDeviceWrite,
        handle as u64,
        start_block,
        buffer.as_ptr() as u64,
        buffer.len() as u64,
    )
    .map(|value| value as usize)
}

pub fn packet_interface_info(handle: Handle) -> Result<PacketInterfaceInfo> {
    let mut info = PacketInterfaceInfo {
        backend: PacketInterfaceBackend::Unknown as u32,
        link_state: PacketInterfaceLinkState::Down as u32,
        mtu: 0,
        rx_ready: 0,
        mac: [0; 6],
        reserved: [0; 2],
        rx_packets: 0,
        tx_packets: 0,
        dropped_packets: 0,
    };
    syscall2(
        SyscallNumber::PacketInterfaceInfo,
        handle as u64,
        &mut info as *mut PacketInterfaceInfo as u64,
    )?;
    Ok(info)
}

pub fn packet_interface_receive(handle: Handle, buffer: &mut [u8]) -> Result<usize> {
    syscall4(
        SyscallNumber::PacketInterfaceReceive,
        handle as u64,
        buffer.as_mut_ptr() as u64,
        buffer.len() as u64,
        0,
    )
    .map(|value| value as usize)
}

pub fn packet_interface_receive_nonblocking(handle: Handle, buffer: &mut [u8]) -> Result<usize> {
    syscall4(
        SyscallNumber::PacketInterfaceReceive,
        handle as u64,
        buffer.as_mut_ptr() as u64,
        buffer.len() as u64,
        PACKET_INTERFACE_FLAG_NONBLOCK as u64,
    )
    .map(|value| value as usize)
}

pub fn packet_interface_transmit(handle: Handle, frame: &[u8]) -> Result<usize> {
    syscall3(
        SyscallNumber::PacketInterfaceTransmit,
        handle as u64,
        frame.as_ptr() as u64,
        frame.len() as u64,
    )
    .map(|value| value as usize)
}

/// Negotiate the shared RX packet ring for this interface. On success
/// returns the memory-object handle to map plus the wire layout; the caller
/// then reads frames in place from its mapping instead of copying each one
/// through PacketInterfaceReceive.
pub fn packet_interface_ring_setup(
    handle: Handle,
    layout: &mut PacketRingLayout,
) -> Result<Handle> {
    let object = syscall2(
        SyscallNumber::PacketInterfaceRingSetup,
        handle as u64,
        layout as *mut PacketRingLayout as u64,
    )?;
    Ok(object as Handle)
}

pub fn display_output_info(handle: Handle) -> Result<DisplayOutputInfo> {
    let mut info = DisplayOutputInfo {
        backend: DisplayOutputBackend::Unknown as u32,
        state: DisplayOutputState::Disconnected as u32,
        pixel_format: DisplayPixelFormat::Unknown as u32,
        reserved: 0,
        width: 0,
        height: 0,
        stride: 0,
        bytes_per_pixel: 0,
        byte_len: 0,
        present_count: 0,
    };
    syscall2(
        SyscallNumber::DisplayOutputInfo,
        handle as u64,
        &mut info as *mut DisplayOutputInfo as u64,
    )?;
    Ok(info)
}

pub fn display_output_present(handle: Handle, frame: &[u8]) -> Result<usize> {
    syscall3(
        SyscallNumber::DisplayOutputPresent,
        handle as u64,
        frame.as_ptr() as u64,
        frame.len() as u64,
    )
    .map(|value| value as usize)
}

pub fn display_output_present_damage(
    handle: Handle,
    frame: &[u8],
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<usize> {
    syscall5(
        SyscallNumber::DisplayOutputPresentDamage,
        handle as u64,
        frame.as_ptr() as u64,
        frame.len() as u64,
        ((y as u32 as u64) << 32) | (x as u32 as u64),
        ((height as u64) << 32) | (width as u64),
    )
    .map(|value| value as usize)
}

pub fn input_source_info(handle: Handle) -> Result<InputSourceInfo> {
    let mut info = InputSourceInfo {
        backend: InputSourceBackend::Unknown as u32,
        capabilities: 0,
        device_count: 0,
        pending_events: 0,
    };
    syscall2(
        SyscallNumber::InputSourceInfo,
        handle as u64,
        &mut info as *mut InputSourceInfo as u64,
    )?;
    Ok(info)
}

pub fn input_source_receive(handle: Handle) -> Result<InputEventInfo> {
    let mut event = InputEventInfo {
        kind: 0,
        code: 0,
        value0: 0,
        value1: 0,
        source_id: 0,
    };
    syscall3(
        SyscallNumber::InputSourceReceive,
        handle as u64,
        &mut event as *mut InputEventInfo as u64,
        0,
    )?;
    Ok(event)
}

pub fn input_source_receive_nonblocking(handle: Handle) -> Result<InputEventInfo> {
    let mut event = InputEventInfo {
        kind: 0,
        code: 0,
        value0: 0,
        value1: 0,
        source_id: 0,
    };
    syscall3(
        SyscallNumber::InputSourceReceive,
        handle as u64,
        &mut event as *mut InputEventInfo as u64,
        INPUT_SOURCE_FLAG_NONBLOCK as u64,
    )?;
    Ok(event)
}

pub fn audio_endpoint_info(handle: Handle) -> Result<AudioEndpointInfo> {
    let mut info = AudioEndpointInfo {
        backend: 0,
        direction: 0,
        state: 0,
        capabilities: 0,
        nominal_rate_hz: 0,
        channels: 0,
        min_frequency_hz: 0,
        max_frequency_hz: 0,
        current_frequency_hz: 0,
        reserved: 0,
        play_count: 0,
    };
    syscall2(
        SyscallNumber::AudioEndpointInfo,
        handle as u64,
        &mut info as *mut AudioEndpointInfo as u64,
    )?;
    Ok(info)
}

pub fn audio_endpoint_play_tone(handle: Handle, request: AudioToneRequest) -> Result<()> {
    let request = request;
    syscall2(
        SyscallNumber::AudioEndpointPlayTone,
        handle as u64,
        &request as *const AudioToneRequest as u64,
    )?;
    Ok(())
}

pub fn audio_endpoint_stop(handle: Handle) -> Result<()> {
    let _ = syscall1(SyscallNumber::AudioEndpointStop, handle as u64)?;
    Ok(())
}

/// Push interleaved s16le stereo PCM frames to the kernel endpoint's
/// playback sink. Returns the number of bytes accepted by the backend.
pub fn audio_endpoint_pcm_write(handle: Handle, bytes: &[u8]) -> Result<usize> {
    let written = syscall3(
        SyscallNumber::AudioEndpointPcmWrite,
        handle as u64,
        bytes.as_ptr() as u64,
        bytes.len() as u64,
    )?;
    Ok(written as usize)
}
