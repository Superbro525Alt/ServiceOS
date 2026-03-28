use crate::{
    syscall2, syscall3, syscall4, DisplayOutputBackend, DisplayOutputInfo, DisplayOutputState,
    DisplayPixelFormat, Handle, InputEventInfo, InputSourceBackend, InputSourceInfo,
    PacketInterfaceBackend, PacketInterfaceInfo, PacketInterfaceLinkState, Result, SyscallNumber,
    INPUT_SOURCE_FLAG_NONBLOCK, PACKET_INTERFACE_FLAG_NONBLOCK,
};

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
    };
    syscall3(
        SyscallNumber::InputSourceReceive,
        handle as u64,
        &mut event as *mut InputEventInfo as u64,
        INPUT_SOURCE_FLAG_NONBLOCK as u64,
    )?;
    Ok(event)
}
