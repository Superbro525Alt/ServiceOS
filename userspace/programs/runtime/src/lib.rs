#![no_std]

use core::{
    arch::asm,
    fmt::{self, Write},
};

pub use serviceos_abi::{
    AppControlTag, AppKeyAction, AppPointerAction, ConfigKey, ConfigTag, ConfigValueKind,
    ConsoleTag, ControlTag, DesktopAppId, DesktopDragMode, DesktopInputAction, DesktopStatus,
    DesktopTag, DesktopWindowAction, DisplayOutputBackend, DisplayOutputInfo,
    DisplayOutputState, DisplayPixelFormat, GraphicsStatus, GraphicsTag, Handle, HandlePair,
    IPC_FLAG_NONBLOCK, IPC_MAX_HANDLES, IPC_MAX_WORDS, INPUT_SOURCE_FLAG_NONBLOCK, INVALID_HANDLE,
    InputButton, InputEventInfo, InputEventKind, InputSourceBackend, InputSourceInfo,
    LifecycleEvent, LogDomain, LogEvent, LogQueryStatus, LogSeverity, LogTag, LookupStatus,
    ManagerAction, ManagerServicePhase, ManagerStatus, ManagerTag, NetworkStatus, NetworkTag,
    PACKET_INTERFACE_FLAG_NONBLOCK, PacketInterfaceBackend, PacketInterfaceInfo,
    PacketInterfaceLinkState, PackageStatus, PackageTag, RawMessage, ServiceId, ServiceImageId,
    SessionInputSource, SessionStatus, SessionTag, StatusTag, StorageStatus, StorageTag,
    SurfaceTag, SyscallErrorCode, SyscallNumber, TaskStateCode, TaskStatus,
};
pub use serviceos_abi::{input_capability, rights};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Unsupported,
    InvalidCall,
    PermissionDenied,
    NotInitialized,
    InvalidArgument,
    BufferTooSmall,
    QueueEmpty,
    NotFound,
    Busy,
    CapacityExceeded,
    Unknown(u64),
}

impl Error {
    fn from_code(code: u64) -> Self {
        match code {
            x if x == SyscallErrorCode::Unsupported as u64 => Self::Unsupported,
            x if x == SyscallErrorCode::InvalidCall as u64 => Self::InvalidCall,
            x if x == SyscallErrorCode::PermissionDenied as u64 => Self::PermissionDenied,
            x if x == SyscallErrorCode::NotInitialized as u64 => Self::NotInitialized,
            x if x == SyscallErrorCode::InvalidArgument as u64 => Self::InvalidArgument,
            x if x == SyscallErrorCode::BufferTooSmall as u64 => Self::BufferTooSmall,
            x if x == SyscallErrorCode::QueueEmpty as u64 => Self::QueueEmpty,
            x if x == SyscallErrorCode::NotFound as u64 => Self::NotFound,
            x if x == SyscallErrorCode::Busy as u64 => Self::Busy,
            x if x == SyscallErrorCode::CapacityExceeded as u64 => Self::CapacityExceeded,
            other => Self::Unknown(other),
        }
    }
}

pub type Result<T> = core::result::Result<T, Error>;

pub fn abi_version() -> Result<u64> {
    syscall0(SyscallNumber::AbiVersion)
}

pub fn monotonic_now() -> Result<u64> {
    syscall0(SyscallNumber::MonotonicNow)
}

pub fn yield_current() -> Result<()> {
    syscall0(SyscallNumber::YieldCurrent).map(|_| ())
}

pub fn thread_exit(code: u64) -> ! {
    let _ = syscall1(SyscallNumber::ThreadExit, code);
    loop {
        core::hint::spin_loop();
    }
}

pub fn debug_log(message: &[u8]) -> Result<()> {
    syscall2(
        SyscallNumber::DebugLogWrite,
        message.as_ptr() as u64,
        message.len() as u64,
    )
    .map(|_| ())
}

pub fn debug_console_read_byte() -> Result<u8> {
    syscall0(SyscallNumber::DebugConsoleRead).map(|value| value as u8)
}

pub fn debug_console_write(bytes: &[u8]) -> Result<()> {
    syscall2(
        SyscallNumber::DebugConsoleWrite,
        bytes.as_ptr() as u64,
        bytes.len() as u64,
    )
    .map(|_| ())
}

pub fn channel_create() -> Result<HandlePair> {
    let mut pair = HandlePair {
        first: INVALID_HANDLE,
        second: INVALID_HANDLE,
    };
    syscall1(SyscallNumber::ChannelCreate, &mut pair as *mut HandlePair as u64)?;
    Ok(pair)
}

pub fn channel_send(endpoint: Handle, message: &RawMessage) -> Result<()> {
    syscall2(
        SyscallNumber::ChannelSend,
        endpoint as u64,
        message as *const RawMessage as u64,
    )
    .map(|_| ())
}

pub fn channel_receive(endpoint: Handle, message: &mut RawMessage) -> Result<()> {
    syscall2(
        SyscallNumber::ChannelReceive,
        endpoint as u64,
        message as *mut RawMessage as u64,
    )
    .map(|_| ())
}

pub fn channel_receive_nonblocking(endpoint: Handle, message: &mut RawMessage) -> Result<()> {
    message.flags = IPC_FLAG_NONBLOCK;
    let result = channel_receive(endpoint, message);
    message.flags = 0;
    result
}

pub fn channel_receive_blocking(endpoint: Handle, message: &mut RawMessage) -> Result<()> {
    loop {
        match channel_receive(endpoint, message) {
            Ok(()) => return Ok(()),
            Err(Error::QueueEmpty) => {}
            Err(error) => return Err(error),
        }
    }
}

pub fn handle_duplicate(handle: Handle, rights: u64) -> Result<Handle> {
    syscall2(
        SyscallNumber::HandleDuplicate,
        handle as u64,
        rights,
    )
    .map(|value| value as Handle)
}

pub fn handle_close(handle: Handle) -> Result<()> {
    syscall1(SyscallNumber::HandleClose, handle as u64).map(|_| ())
}

pub fn service_spawn(
    image_id: ServiceImageId,
    bootstrap_authority: Handle,
    bootstrap_handle: Handle,
) -> Result<Handle> {
    syscall3(
        SyscallNumber::ServiceSpawn,
        image_id as u32 as u64,
        bootstrap_authority as u64,
        bootstrap_handle as u64,
    )
    .map(|value| value as Handle)
}

pub fn task_status(task_handle: Handle) -> Result<TaskStatus> {
    let mut status = TaskStatus {
        state: TaskStateCode::Running,
        exit_code: 0,
    };
    syscall2(
        SyscallNumber::TaskStatus,
        task_handle as u64,
        &mut status as *mut TaskStatus as u64,
    )?;
    Ok(status)
}

pub fn memory_read(handle: Handle, offset: usize, buffer: &mut [u8]) -> Result<usize> {
    syscall4(
        SyscallNumber::MemoryRead,
        handle as u64,
        offset as u64,
        buffer.as_mut_ptr() as u64,
        buffer.len() as u64,
    )
    .map(|value| value as usize)
}

pub fn memory_create(size_bytes: usize, writable: bool) -> Result<Handle> {
    syscall2(
        SyscallNumber::MemoryCreate,
        size_bytes as u64,
        u64::from(writable),
    )
    .map(|value| value as Handle)
}

pub fn memory_write(handle: Handle, offset: usize, bytes: &[u8]) -> Result<usize> {
    syscall4(
        SyscallNumber::MemoryWrite,
        handle as u64,
        offset as u64,
        bytes.as_ptr() as u64,
        bytes.len() as u64,
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

pub fn wait_for_exit(task_handle: Handle) -> Result<TaskStatus> {
    loop {
        let status = task_status(task_handle)?;
        if status.state == TaskStateCode::Exited {
            return Ok(status);
        }
        yield_current()?;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LogRecord {
    pub sequence: u64,
    pub source: ServiceId,
    pub severity: LogSeverity,
    pub domain: LogDomain,
    pub event: LogEvent,
    pub arg0: u64,
    pub arg1: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ManagerServiceInfo {
    pub service_id: ServiceId,
    pub phase: ManagerServicePhase,
    pub attempts: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PackageListEntry {
    pub service_id: ServiceId,
    pub installed: bool,
    pub active: bool,
    pub rollback_available: bool,
    pub repository_versions: u32,
    pub installed_version_len: usize,
    pub active_version_len: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PackageInfo {
    pub installed: bool,
    pub active: bool,
    pub rollback_available: bool,
    pub repository_versions: u32,
    pub installed_version_len: usize,
    pub active_version_len: usize,
    pub rollback_version_len: usize,
    pub latest_version_len: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkInterfaceStatusInfo {
    pub index: u32,
    pub backend: PacketInterfaceBackend,
    pub link_state: PacketInterfaceLinkState,
    pub mtu: u32,
    pub address: u32,
    pub prefix_len: u8,
    pub gateway: u32,
    pub mac: [u8; 6],
    pub rx_packets: u64,
    pub tx_packets: u64,
    pub dropped_packets: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GraphicsOutputStatusInfo {
    pub index: u32,
    pub backend: DisplayOutputBackend,
    pub state: DisplayOutputState,
    pub pixel_format: DisplayPixelFormat,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub bytes_per_pixel: u32,
    pub byte_len: u64,
    pub present_count: u64,
    pub surface_count: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GraphicsSurfaceStatusInfo {
    pub surface_id: u32,
    pub output_index: u32,
    pub owner_session: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub z_order: u32,
    pub fill_rgb: u32,
    pub visible: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionStatusInfo {
    pub session_id: u32,
    pub input_source: SessionInputSource,
    pub focused_surface: u32,
    pub surface_count: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DesktopShellStatusInfo {
    pub session_id: u32,
    pub focused_app: Option<DesktopAppId>,
    pub running_apps: u32,
    pub focused_surface: u32,
    pub drag_mode: DesktopDragMode,
    pub pointer_x: i32,
    pub pointer_y: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DesktopAppInfo {
    pub app_id: DesktopAppId,
    pub running: bool,
    pub focused: bool,
    pub surface_id: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DesktopWindowInfo {
    pub app_id: DesktopAppId,
    pub surface_id: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub z_order: u32,
    pub focused: bool,
    pub minimized: bool,
    pub visible: bool,
}

pub fn register_service(bootstrap: Handle, service_id: ServiceId, public: Handle) -> Result<()> {
    let mut register = RawMessage::empty(ControlTag::Register as u32);
    register.word_count = 1;
    register.words[0] = service_id as u32 as u64;
    register.handle_count = 1;
    register.handles[0] = public;
    register.handle_rights[0] =
        rights::SEND | rights::RECEIVE | rights::DUPLICATE | rights::TRANSFER;
    channel_send(bootstrap, &register)
}

pub fn lookup_service(bootstrap: Handle, service_id: ServiceId) -> Result<Handle> {
    let mut request = RawMessage::empty(ControlTag::LookupRequest as u32);
    request.word_count = 1;
    request.words[0] = service_id as u32 as u64;
    channel_send(bootstrap, &request)?;

    let mut reply = RawMessage::empty(0);
    channel_receive_blocking(bootstrap, &mut reply)?;
    if reply.tag != ControlTag::LookupReply as u32 || reply.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    match reply.words[1] as u32 {
        x if x == LookupStatus::Ok as u32 && reply.handle_count > 0 => Ok(reply.handles[0]),
        x if x == LookupStatus::Denied as u32 => Err(Error::PermissionDenied),
        x if x == LookupStatus::Unavailable as u32 => Err(Error::NotFound),
        _ => Err(Error::InvalidArgument),
    }
}

pub fn send_log_record(
    log_handle: Handle,
    source: ServiceId,
    severity: LogSeverity,
    domain: LogDomain,
    event: LogEvent,
    arg0: u64,
    arg1: u64,
) -> Result<()> {
    let mut message = RawMessage::empty(LogTag::Record as u32);
    message.word_count = 6;
    message.words[0] = source as u32 as u64;
    message.words[1] = severity as u32 as u64;
    message.words[2] = domain as u32 as u64;
    message.words[3] = event as u32 as u64;
    message.words[4] = arg0;
    message.words[5] = arg1;
    channel_send(log_handle, &message)
}

pub fn console_write_record(
    console_handle: Handle,
    source: ServiceId,
    severity: LogSeverity,
    domain: LogDomain,
    event: LogEvent,
    arg0: u64,
    arg1: u64,
    sequence: u64,
) -> Result<()> {
    let mut message = RawMessage::empty(ConsoleTag::WriteRecord as u32);
    message.word_count = 7;
    message.words[0] = source as u32 as u64;
    message.words[1] = severity as u32 as u64;
    message.words[2] = domain as u32 as u64;
    message.words[3] = event as u32 as u64;
    message.words[4] = arg0;
    message.words[5] = arg1;
    message.words[6] = sequence;
    channel_send(console_handle, &message)
}

pub fn console_session_open(console_handle: Handle) -> Result<Handle> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(ConsoleTag::SessionOpenRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(console_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != ConsoleTag::SessionOpenReply as u32 || response.handle_count < 1 {
        return Err(Error::Busy);
    }
    Ok(response.handles[0])
}

pub fn console_session_write(session_handle: Handle, text: &str) -> Result<()> {
    let text_bytes = text.as_bytes();
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(1)) * 8;
    if text_bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }
    let mut message = RawMessage::empty(ConsoleTag::SessionWriteText as u32);
    message.word_count = 1 + pack_bytes(text_bytes, &mut message.words[1..])?;
    message.words[0] = text_bytes.len() as u64;
    channel_send(session_handle, &message)
}

pub fn console_session_read_line(session_handle: Handle, buffer: &mut [u8]) -> Result<usize> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(ConsoleTag::SessionReadLineRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(session_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != ConsoleTag::SessionReadLineReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    let len = response.words[0] as usize;
    unpack_bytes(&response.words[1..response.word_count as usize], len, buffer)?;
    Ok(len)
}

pub fn config_read(config_handle: Handle, key: ConfigKey) -> Result<(ConfigValueKind, u64)> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(ConfigTag::ReadRequest as u32);
    request.word_count = 1;
    request.words[0] = key as u32 as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(config_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != ConfigTag::ReadReply as u32 || response.word_count < 3 {
        return Err(Error::InvalidArgument);
    }
    let kind = match response.words[1] as u32 {
        x if x == ConfigValueKind::Unsigned as u32 => ConfigValueKind::Unsigned,
        _ => return Err(Error::InvalidArgument),
    };
    Ok((kind, response.words[2]))
}

pub fn storage_open(storage_handle: Handle, path: &str) -> Result<(Handle, usize)> {
    let path_bytes = path.as_bytes();
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(1)) * 8;
    if path_bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let reply = channel_create()?;
    let mut request = RawMessage::empty(StorageTag::OpenRequest as u32);
    request.word_count = 1 + pack_bytes(path_bytes, &mut request.words[1..])?;
    request.words[0] = path_bytes.len() as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(storage_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != StorageTag::OpenReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }

    match response.words[0] as u32 {
        x if x == StorageStatus::Ok as u32 && response.handle_count > 0 => {
            Ok((response.handles[0], response.words[1] as usize))
        }
        x if x == StorageStatus::NotFound as u32 => Err(Error::NotFound),
        x if x == StorageStatus::Busy as u32 => Err(Error::Busy),
        x if x == StorageStatus::InvalidPath as u32 => Err(Error::InvalidArgument),
        _ => Err(Error::InvalidArgument),
    }
}

pub fn storage_read(blob_handle: Handle, offset: usize, buffer: &mut [u8]) -> Result<usize> {
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(3)) * 8;
    let requested = buffer.len().min(max_inline_bytes);
    let reply = channel_create()?;
    let mut request = RawMessage::empty(StorageTag::ReadRequest as u32);
    request.word_count = 2;
    request.words[0] = offset as u64;
    request.words[1] = requested as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(blob_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != StorageTag::ReadReply as u32 || response.word_count < 3 {
        return Err(Error::InvalidArgument);
    }

    match response.words[0] as u32 {
        x if x == StorageStatus::Ok as u32 => {
            let byte_len = response.words[2] as usize;
            if byte_len > requested || byte_len > buffer.len() {
                return Err(Error::BufferTooSmall);
            }
            unpack_bytes(&response.words[3..response.word_count as usize], byte_len, buffer)?;
            Ok(byte_len)
        }
        x if x == StorageStatus::InvalidOffset as u32 => Err(Error::InvalidArgument),
        x if x == StorageStatus::NotFound as u32 => Err(Error::NotFound),
        _ => Err(Error::InvalidArgument),
    }
}

pub fn storage_blob_close(blob_handle: Handle) -> Result<()> {
    let request = RawMessage::empty(StorageTag::CloseRequest as u32);
    let _ = channel_send(blob_handle, &request);
    handle_close(blob_handle)
}

pub fn storage_list(
    storage_handle: Handle,
    prefix: &str,
    index: usize,
    path_buffer: &mut [u8],
) -> Result<Option<(StorageStatus, usize)>> {
    let prefix_bytes = prefix.as_bytes();
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(2)) * 8;
    if prefix_bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let reply = channel_create()?;
    let mut request = RawMessage::empty(StorageTag::ListRequest as u32);
    request.word_count = 2 + pack_bytes(prefix_bytes, &mut request.words[2..])?;
    request.words[0] = index as u64;
    request.words[1] = prefix_bytes.len() as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(storage_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != StorageTag::ListReply as u32 || response.word_count < 3 {
        return Err(Error::InvalidArgument);
    }

    let status = match response.words[0] as u32 {
        x if x == StorageStatus::Ok as u32 => StorageStatus::Ok,
        x if x == StorageStatus::End as u32 => StorageStatus::End,
        x if x == StorageStatus::Busy as u32 => StorageStatus::Busy,
        x if x == StorageStatus::InvalidPath as u32 => StorageStatus::InvalidPath,
        _ => return Err(Error::InvalidArgument),
    };
    if status == StorageStatus::End {
        return Ok(None);
    }

    let path_len = response.words[2] as usize;
    unpack_bytes(&response.words[3..response.word_count as usize], path_len, path_buffer)?;
    Ok(Some((status, path_len)))
}

pub fn log_query_info(log_handle: Handle) -> Result<(u64, u64)> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(LogTag::QueryInfoRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(log_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != LogTag::QueryInfoReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    Ok((response.words[0], response.words[1]))
}

pub fn log_query_record(log_handle: Handle, sequence: u64) -> Result<Option<LogRecord>> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(LogTag::QueryRecordRequest as u32);
    request.word_count = 1;
    request.words[0] = sequence;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(log_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != LogTag::QueryRecordReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    if response.words[0] as u32 == LogQueryStatus::NotFound as u32 {
        return Ok(None);
    }
    if response.word_count < 8 {
        return Err(Error::InvalidArgument);
    }

    Ok(Some(LogRecord {
        sequence: response.words[1],
        source: service_id_from_word(response.words[2]),
        severity: severity_from_word(response.words[3]),
        domain: domain_from_word(response.words[4]),
        event: event_from_word(response.words[5]),
        arg0: response.words[6],
        arg1: response.words[7],
    }))
}

pub fn manager_list_services(
    bootstrap: Handle,
    services: &mut [ManagerServiceInfo],
) -> Result<usize> {
    let mut loaded = 0usize;
    let mut page = 0usize;

    loop {
        let mut request = RawMessage::empty(ManagerTag::ListServicesRequest as u32);
        request.word_count = 1;
        request.words[0] = page as u64;
        channel_send(bootstrap, &request)?;

        let mut response = RawMessage::empty(0);
        channel_receive_blocking(bootstrap, &mut response)?;
        if response.tag != ManagerTag::ListServicesReply as u32 || response.word_count < 2 {
            return Err(Error::InvalidArgument);
        }

        let count = response.words[0] as usize;
        let next_page = response.words[1] as usize;
        if loaded + count > services.len() || response.word_count < (2 + count * 2) as u32 {
            return Err(Error::BufferTooSmall);
        }

        for index in 0..count {
            services[loaded + index] = ManagerServiceInfo {
                service_id: service_id_from_word(response.words[2 + index * 2]),
                phase: manager_phase_from_word(response.words[3 + index * 2]),
                attempts: (response.words[3 + index * 2] >> 32) as u32,
            };
        }
        loaded += count;

        if next_page == usize::MAX {
            break;
        }
        page = next_page;
    }

    Ok(loaded)
}

pub fn manager_service_status(
    bootstrap: Handle,
    service_id: ServiceId,
) -> Result<(ManagerStatus, ManagerServicePhase, u32, u64)> {
    let mut request = RawMessage::empty(ManagerTag::ServiceStatusRequest as u32);
    request.word_count = 1;
    request.words[0] = service_id as u32 as u64;
    channel_send(bootstrap, &request)?;

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(bootstrap, &mut response)?;
    if response.tag != ManagerTag::ServiceStatusReply as u32 || response.word_count < 4 {
        return Err(Error::InvalidArgument);
    }

    Ok((
        manager_status_from_word(response.words[0]),
        manager_phase_from_word(response.words[1]),
        response.words[2] as u32,
        response.words[3],
    ))
}

pub fn manager_restart_service(bootstrap: Handle, service_id: ServiceId) -> Result<()> {
    let mut request = RawMessage::empty(ManagerTag::ServiceActionRequest as u32);
    request.word_count = 2;
    request.words[0] = service_id as u32 as u64;
    request.words[1] = ManagerAction::Restart as u32 as u64;
    channel_send(bootstrap, &request)?;

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(bootstrap, &mut response)?;
    if response.tag != ManagerTag::ServiceActionReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match manager_status_from_word(response.words[0]) {
        ManagerStatus::Ok => Ok(()),
        ManagerStatus::Busy | ManagerStatus::Failed => Err(Error::Busy),
        ManagerStatus::NotFound => Err(Error::NotFound),
        ManagerStatus::Denied => Err(Error::PermissionDenied),
    }
}

pub fn manager_launch_program(
    bootstrap: Handle,
    image_id: ServiceImageId,
    io_handle: Option<Handle>,
) -> Result<Handle> {
    match io_handle {
        Some(handle) => manager_launch_program_with_payload(
            bootstrap,
            image_id,
            &[1],
            &[StartupHandle {
                handle,
                rights: rights::SEND | rights::RECEIVE | rights::DUPLICATE | rights::TRANSFER,
            }],
        ),
        None => manager_launch_program_with_payload(bootstrap, image_id, &[0], &[]),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StartupHandle {
    pub handle: Handle,
    pub rights: u64,
}

pub fn manager_launch_program_with_payload(
    bootstrap: Handle,
    image_id: ServiceImageId,
    startup_words: &[u64],
    startup_handles: &[StartupHandle],
) -> Result<Handle> {
    if startup_words.len() + 2 > IPC_MAX_WORDS || startup_handles.len() > IPC_MAX_HANDLES {
        return Err(Error::BufferTooSmall);
    }
    let mut request = RawMessage::empty(ManagerTag::LaunchRequest as u32);
    request.word_count = 2 + startup_words.len() as u32;
    request.words[0] = image_id as u32 as u64;
    request.words[1] = startup_words.len() as u64;
    for (index, word) in startup_words.iter().copied().enumerate() {
        request.words[2 + index] = word;
    }
    for (index, startup_handle) in startup_handles.iter().copied().enumerate() {
        request.handles[index] = startup_handle.handle;
        request.handle_rights[index] = startup_handle.rights;
    }
    request.handle_count = startup_handles.len() as u32;
    channel_send(bootstrap, &request)?;

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(bootstrap, &mut response)?;
    if response.tag != ManagerTag::LaunchReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match manager_status_from_word(response.words[0]) {
        ManagerStatus::Ok if response.handle_count > 0 => Ok(response.handles[0]),
        ManagerStatus::Busy | ManagerStatus::Failed => Err(Error::Busy),
        ManagerStatus::NotFound => Err(Error::NotFound),
        ManagerStatus::Denied => Err(Error::PermissionDenied),
        _ => Err(Error::InvalidArgument),
    }
}

pub fn desktop_status(desktop_handle: Handle) -> Result<DesktopShellStatusInfo> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(DesktopTag::StatusRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(desktop_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != DesktopTag::StatusReply as u32 || response.word_count < 4 {
        return Err(Error::InvalidArgument);
    }
    match desktop_status_from_word(response.words[0]) {
        DesktopStatus::Ok => Ok(DesktopShellStatusInfo {
            session_id: response.words[1] as u32,
            focused_app: desktop_app_id_from_word(response.words[2]).ok(),
            running_apps: response.words[3] as u32,
            focused_surface: response.words.get(4).copied().unwrap_or(0) as u32,
            drag_mode: response
                .words
                .get(5)
                .copied()
                .map(desktop_drag_mode_from_word)
                .unwrap_or(DesktopDragMode::None),
            pointer_x: response
                .words
                .get(6)
                .copied()
                .map(|value| unpack_i32_pair(value).0)
                .unwrap_or(0),
            pointer_y: response
                .words
                .get(6)
                .copied()
                .map(|value| unpack_i32_pair(value).1)
                .unwrap_or(0),
        }),
        status => Err(desktop_status_error(status)),
    }
}

pub fn desktop_list_apps(desktop_handle: Handle, apps: &mut [DesktopAppInfo]) -> Result<usize> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(DesktopTag::ListAppsRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(desktop_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != DesktopTag::ListAppsReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    match desktop_status_from_word(response.words[0]) {
        DesktopStatus::Ok => {}
        status => return Err(desktop_status_error(status)),
    }
    let count = response.words[1] as usize;
    if count > apps.len() || response.word_count as usize != 2 + count * 4 {
        return Err(Error::BufferTooSmall);
    }
    for (index, app) in apps.iter_mut().enumerate().take(count) {
        let base = 2 + index * 4;
        *app = DesktopAppInfo {
            app_id: desktop_app_id_from_word(response.words[base])
                .map_err(|_| Error::InvalidArgument)?,
            running: response.words[base + 1] != 0,
            focused: response.words[base + 2] != 0,
            surface_id: response.words[base + 3] as u32,
        };
    }
    Ok(count)
}

pub fn desktop_launch_app(desktop_handle: Handle, app_id: DesktopAppId) -> Result<u32> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(DesktopTag::LaunchAppRequest as u32);
    request.word_count = 1;
    request.words[0] = app_id as u32 as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(desktop_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != DesktopTag::LaunchAppReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    match desktop_status_from_word(response.words[0]) {
        DesktopStatus::Ok => Ok(response.words[1] as u32),
        status => Err(desktop_status_error(status)),
    }
}

pub fn desktop_focus_app(desktop_handle: Handle, app_id: DesktopAppId) -> Result<u32> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(DesktopTag::FocusAppRequest as u32);
    request.word_count = 1;
    request.words[0] = app_id as u32 as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(desktop_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != DesktopTag::FocusAppReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    match desktop_status_from_word(response.words[0]) {
        DesktopStatus::Ok => Ok(response.words[1] as u32),
        status => Err(desktop_status_error(status)),
    }
}

pub fn desktop_list_windows(
    desktop_handle: Handle,
    windows: &mut [DesktopWindowInfo],
) -> Result<usize> {
    let mut filled = 0usize;
    let mut start = 0u32;

    loop {
        let reply = channel_create()?;
        let mut request = RawMessage::empty(DesktopTag::ListWindowsRequest as u32);
        request.word_count = 1;
        request.words[0] = start as u64;
        request.handle_count = 1;
        request.handles[0] = reply.second;
        request.handle_rights[0] = rights::SEND;
        channel_send(desktop_handle, &request)?;
        let _ = handle_close(reply.second);

        let mut response = RawMessage::empty(0);
        channel_receive_blocking(reply.first, &mut response)?;
        let _ = handle_close(reply.first);
        if response.tag != DesktopTag::ListWindowsReply as u32 || response.word_count < 3 {
            return Err(Error::InvalidArgument);
        }
        match desktop_status_from_word(response.words[0]) {
            DesktopStatus::Ok => {}
            status => return Err(desktop_status_error(status)),
        }

        let count = response.words[1] as usize;
        let next_start = response.words[2] as u32;
        if response.word_count as usize != 3 + count * 5 {
            return Err(Error::InvalidArgument);
        }
        if filled + count > windows.len() {
            return Err(Error::BufferTooSmall);
        }

        for index in 0..count {
            let base = 3 + index * 5;
            let flags = response.words[base + 2];
            let (x, y) = unpack_i32_pair(response.words[base + 3]);
            let (width, height) = unpack_u32_pair(response.words[base + 4]);
            windows[filled + index] = DesktopWindowInfo {
                app_id: desktop_app_id_from_word(response.words[base])
                    .map_err(|_| Error::InvalidArgument)?,
                surface_id: response.words[base + 1] as u32,
                x,
                y,
                width,
                height,
                z_order: (flags >> 32) as u32,
                focused: (flags & 0x1) != 0,
                minimized: (flags & 0x2) != 0,
                visible: (flags & 0x4) != 0,
            };
        }
        filled += count;
        if next_start == u32::MAX {
            break;
        }
        start = next_start;
    }

    Ok(filled)
}

pub fn desktop_window_action(
    desktop_handle: Handle,
    action: DesktopWindowAction,
    app_id: Option<DesktopAppId>,
    arg0: u64,
    arg1: u64,
) -> Result<u32> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(DesktopTag::WindowActionRequest as u32);
    request.word_count = 4;
    request.words[0] = action as u32 as u64;
    request.words[1] = app_id.map(|value| value as u32 as u64).unwrap_or(0);
    request.words[2] = arg0;
    request.words[3] = arg1;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(desktop_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != DesktopTag::WindowActionReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    match desktop_status_from_word(response.words[0]) {
        DesktopStatus::Ok => Ok(response.words[1] as u32),
        status => Err(desktop_status_error(status)),
    }
}

pub fn desktop_focus_next(desktop_handle: Handle) -> Result<u32> {
    desktop_window_action(desktop_handle, DesktopWindowAction::FocusNext, None, 0, 0)
}

pub fn desktop_close_app(desktop_handle: Handle, app_id: DesktopAppId) -> Result<()> {
    let _ = desktop_window_action(
        desktop_handle,
        DesktopWindowAction::Close,
        Some(app_id),
        0,
        0,
    )?;
    Ok(())
}

pub fn desktop_minimize_app(desktop_handle: Handle, app_id: DesktopAppId) -> Result<u32> {
    desktop_window_action(
        desktop_handle,
        DesktopWindowAction::Minimize,
        Some(app_id),
        0,
        0,
    )
}

pub fn desktop_restore_app(desktop_handle: Handle, app_id: DesktopAppId) -> Result<u32> {
    desktop_window_action(
        desktop_handle,
        DesktopWindowAction::Restore,
        Some(app_id),
        0,
        0,
    )
}

pub fn desktop_maximize_app(desktop_handle: Handle, app_id: DesktopAppId) -> Result<u32> {
    desktop_window_action(
        desktop_handle,
        DesktopWindowAction::Maximize,
        Some(app_id),
        0,
        0,
    )
}

pub fn desktop_move_app(
    desktop_handle: Handle,
    app_id: DesktopAppId,
    x: i32,
    y: i32,
) -> Result<u32> {
    desktop_window_action(
        desktop_handle,
        DesktopWindowAction::Move,
        Some(app_id),
        x as i64 as u64,
        y as i64 as u64,
    )
}

pub fn desktop_resize_app(
    desktop_handle: Handle,
    app_id: DesktopAppId,
    width: u32,
    height: u32,
) -> Result<u32> {
    desktop_window_action(
        desktop_handle,
        DesktopWindowAction::Resize,
        Some(app_id),
        width as u64,
        height as u64,
    )
}

fn desktop_input_request(
    desktop_handle: Handle,
    action: DesktopInputAction,
    x: i32,
    y: i32,
    expect_reply: bool,
) -> Result<Option<u32>> {
    let mut request = RawMessage::empty(DesktopTag::InputRequest as u32);
    request.word_count = 3;
    request.words[0] = action as u32 as u64;
    request.words[1] = x as i64 as u64;
    request.words[2] = y as i64 as u64;
    let mut reply = None;
    if expect_reply {
        let pair = channel_create()?;
        request.handle_count = 1;
        request.handles[0] = pair.second;
        request.handle_rights[0] = rights::SEND;
        reply = Some(pair);
    }
    channel_send(desktop_handle, &request)?;
    if !expect_reply {
        return Ok(None);
    }

    let reply = reply.expect("reply pair for reply-expected desktop input");
    let _ = handle_close(reply.second);
    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != DesktopTag::InputReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    match desktop_status_from_word(response.words[0]) {
        DesktopStatus::Ok => Ok(Some(response.words[1] as u32)),
        status => Err(desktop_status_error(status)),
    }
}

pub fn desktop_pointer_input(
    desktop_handle: Handle,
    action: DesktopInputAction,
    x: i32,
    y: i32,
) -> Result<u32> {
    desktop_input_request(desktop_handle, action, x, y, true).map(|surface| surface.unwrap_or(0))
}

pub fn desktop_pointer_input_async(
    desktop_handle: Handle,
    action: DesktopInputAction,
    x: i32,
    y: i32,
) -> Result<()> {
    desktop_input_request(desktop_handle, action, x, y, false).map(|_| ())
}

pub fn desktop_pointer_click(desktop_handle: Handle, x: i32, y: i32) -> Result<u32> {
    desktop_pointer_input(desktop_handle, DesktopInputAction::Click, x, y)
}

pub fn desktop_key_input(
    desktop_handle: Handle,
    action: DesktopInputAction,
    key_code: u32,
    value: u32,
) -> Result<u32> {
    desktop_pointer_input(desktop_handle, action, key_code as i32, value as i32)
}

pub fn desktop_key_input_async(
    desktop_handle: Handle,
    action: DesktopInputAction,
    key_code: u32,
    value: u32,
) -> Result<()> {
    desktop_pointer_input_async(desktop_handle, action, key_code as i32, value as i32)
}

pub fn manager_activate_service(bootstrap: Handle, manifest_path: &str) -> Result<ServiceId> {
    let path_bytes = manifest_path.as_bytes();
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(1)) * 8;
    if path_bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let mut request = RawMessage::empty(ManagerTag::ActivateRequest as u32);
    request.word_count = 1 + pack_bytes(path_bytes, &mut request.words[1..])?;
    request.words[0] = path_bytes.len() as u64;
    channel_send(bootstrap, &request)?;

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(bootstrap, &mut response)?;
    if response.tag != ManagerTag::ActivateReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }

    match manager_status_from_word(response.words[0]) {
        ManagerStatus::Ok => Ok(service_id_from_word(response.words[1])),
        ManagerStatus::Busy | ManagerStatus::Failed => Err(Error::Busy),
        ManagerStatus::NotFound => Err(Error::NotFound),
        ManagerStatus::Denied => Err(Error::PermissionDenied),
    }
}

pub fn manager_deactivate_service(bootstrap: Handle, service_id: ServiceId) -> Result<()> {
    let mut request = RawMessage::empty(ManagerTag::DeactivateRequest as u32);
    request.word_count = 1;
    request.words[0] = service_id as u32 as u64;
    channel_send(bootstrap, &request)?;

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(bootstrap, &mut response)?;
    if response.tag != ManagerTag::DeactivateReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }

    match manager_status_from_word(response.words[0]) {
        ManagerStatus::Ok => Ok(()),
        ManagerStatus::Busy | ManagerStatus::Failed => Err(Error::Busy),
        ManagerStatus::NotFound => Err(Error::NotFound),
        ManagerStatus::Denied => Err(Error::PermissionDenied),
    }
}

pub fn package_list(
    package_handle: Handle,
    index: usize,
    installed_version: &mut [u8],
    active_version: &mut [u8],
) -> Result<Option<PackageListEntry>> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(PackageTag::ListRequest as u32);
    request.word_count = 1;
    request.words[0] = index as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(package_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != PackageTag::ListReply as u32 || response.word_count < 7 {
        return Err(Error::InvalidArgument);
    }

    let status = package_status_from_word(response.words[0]);
    if status == PackageStatus::End {
        return Ok(None);
    }
    if status != PackageStatus::Ok {
        return Err(package_status_error(status));
    }

    let installed_len = response.words[4] as usize;
    let active_len = response.words[5] as usize;
    let total_bytes = installed_len + active_len;
    let total_words = total_bytes.div_ceil(8);
    if response.word_count as usize != 7 + total_words {
        return Err(Error::InvalidArgument);
    }

    let mut combined = [0u8; IPC_MAX_WORDS * 8];
    unpack_bytes(
        &response.words[7..response.word_count as usize],
        total_bytes,
        &mut combined,
    )?;
    installed_version[..installed_len].copy_from_slice(&combined[..installed_len]);
    active_version[..active_len]
        .copy_from_slice(&combined[installed_len..installed_len + active_len]);

    Ok(Some(PackageListEntry {
        service_id: service_id_from_word(response.words[1]),
        installed: response.words[2] & 1 != 0,
        active: response.words[2] & 2 != 0,
        rollback_available: response.words[2] & 4 != 0,
        repository_versions: response.words[3] as u32,
        installed_version_len: installed_len,
        active_version_len: active_len,
    }))
}

pub fn package_info(
    package_handle: Handle,
    service_id: ServiceId,
    installed_version: &mut [u8],
    active_version: &mut [u8],
    rollback_version: &mut [u8],
    latest_version: &mut [u8],
) -> Result<PackageInfo> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(PackageTag::InfoRequest as u32);
    request.word_count = 1;
    request.words[0] = service_id as u32 as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(package_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != PackageTag::InfoReply as u32 || response.word_count < 8 {
        return Err(Error::InvalidArgument);
    }

    let status = package_status_from_word(response.words[0]);
    if status != PackageStatus::Ok {
        return Err(package_status_error(status));
    }

    let installed_len = response.words[3] as usize;
    let active_len = response.words[4] as usize;
    let rollback_len = response.words[5] as usize;
    let latest_len = response.words[6] as usize;
    let total_bytes = installed_len + active_len + rollback_len + latest_len;
    let total_words = total_bytes.div_ceil(8);
    if response.word_count as usize != 8 + total_words {
        return Err(Error::InvalidArgument);
    }

    let mut combined = [0u8; IPC_MAX_WORDS * 8];
    unpack_bytes(
        &response.words[8..response.word_count as usize],
        total_bytes,
        &mut combined,
    )?;

    let mut offset = 0usize;
    installed_version[..installed_len].copy_from_slice(&combined[offset..offset + installed_len]);
    offset += installed_len;
    active_version[..active_len].copy_from_slice(&combined[offset..offset + active_len]);
    offset += active_len;
    rollback_version[..rollback_len].copy_from_slice(&combined[offset..offset + rollback_len]);
    offset += rollback_len;
    latest_version[..latest_len].copy_from_slice(&combined[offset..offset + latest_len]);

    Ok(PackageInfo {
        installed: response.words[1] & 1 != 0,
        active: response.words[1] & 2 != 0,
        rollback_available: response.words[1] & 4 != 0,
        repository_versions: response.words[2] as u32,
        installed_version_len: installed_len,
        active_version_len: active_len,
        rollback_version_len: rollback_len,
        latest_version_len: latest_len,
    })
}

pub fn package_install(
    package_handle: Handle,
    service_id: ServiceId,
    version: Option<&str>,
) -> Result<()> {
    package_mutation(package_handle, PackageTag::InstallRequest, PackageTag::InstallReply, service_id, version)
}

pub fn package_update(
    package_handle: Handle,
    service_id: ServiceId,
    version: Option<&str>,
) -> Result<()> {
    package_mutation(package_handle, PackageTag::UpdateRequest, PackageTag::UpdateReply, service_id, version)
}

pub fn package_remove(package_handle: Handle, service_id: ServiceId) -> Result<()> {
    package_mutation(
        package_handle,
        PackageTag::RemoveRequest,
        PackageTag::RemoveReply,
        service_id,
        None,
    )
}

pub fn package_rollback(package_handle: Handle, service_id: ServiceId) -> Result<()> {
    package_mutation(
        package_handle,
        PackageTag::RollbackRequest,
        PackageTag::RollbackReply,
        service_id,
        None,
    )
}

pub fn package_history(
    package_handle: Handle,
    service_id: ServiceId,
    current_version: &mut [u8],
    previous_version: &mut [u8],
) -> Result<(usize, usize)> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(PackageTag::HistoryRequest as u32);
    request.word_count = 1;
    request.words[0] = service_id as u32 as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(package_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != PackageTag::HistoryReply as u32 || response.word_count < 4 {
        return Err(Error::InvalidArgument);
    }

    let status = package_status_from_word(response.words[0]);
    if status != PackageStatus::Ok {
        return Err(package_status_error(status));
    }

    let current_len = response.words[1] as usize;
    let previous_len = response.words[2] as usize;
    let total_bytes = current_len + previous_len;
    let total_words = total_bytes.div_ceil(8);
    if response.word_count as usize != 4 + total_words {
        return Err(Error::InvalidArgument);
    }

    let mut combined = [0u8; IPC_MAX_WORDS * 8];
    unpack_bytes(
        &response.words[4..response.word_count as usize],
        total_bytes,
        &mut combined,
    )?;
    current_version[..current_len].copy_from_slice(&combined[..current_len]);
    previous_version[..previous_len]
        .copy_from_slice(&combined[current_len..current_len + previous_len]);
    Ok((current_len, previous_len))
}

fn package_mutation(
    package_handle: Handle,
    request_tag: PackageTag,
    reply_tag: PackageTag,
    service_id: ServiceId,
    version: Option<&str>,
) -> Result<()> {
    let version_bytes = version.unwrap_or("").as_bytes();
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(2)) * 8;
    if version_bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let reply = channel_create()?;
    let mut request = RawMessage::empty(request_tag as u32);
    request.word_count = 2 + pack_bytes(version_bytes, &mut request.words[2..])?;
    request.words[0] = service_id as u32 as u64;
    request.words[1] = version_bytes.len() as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(package_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != reply_tag as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }

    match package_status_from_word(response.words[0]) {
        PackageStatus::Ok => Ok(()),
        status => Err(package_status_error(status)),
    }
}

pub fn status_snapshot(status_handle: Handle) -> Result<(u64, u64)> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(StatusTag::SnapshotRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(status_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != StatusTag::SnapshotReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    Ok((response.words[0], response.words[1]))
}

pub fn network_interface_count(network_handle: Handle) -> Result<usize> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(NetworkTag::InterfaceListRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(network_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != NetworkTag::InterfaceListReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    match network_status_from_word(response.words[0]) {
        NetworkStatus::Ok => Ok(response.words[1] as usize),
        NetworkStatus::Busy => Err(Error::Busy),
        NetworkStatus::Unsupported => Err(Error::Unsupported),
        _ => Err(Error::InvalidArgument),
    }
}

pub fn network_interface_status(
    network_handle: Handle,
    index: usize,
) -> Result<Option<NetworkInterfaceStatusInfo>> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(NetworkTag::InterfaceStatusRequest as u32);
    request.word_count = 1;
    request.words[0] = index as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(network_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != NetworkTag::InterfaceStatusReply as u32 || response.word_count < 12 {
        return Err(Error::InvalidArgument);
    }

    let status = network_status_from_word(response.words[0]);
    if status == NetworkStatus::NotFound {
        return Ok(None);
    }
    if status != NetworkStatus::Ok {
        return Err(network_status_error(status));
    }

    Ok(Some(NetworkInterfaceStatusInfo {
        index: response.words[1] as u32,
        backend: packet_backend_from_word(response.words[2]),
        link_state: packet_link_state_from_word(response.words[3]),
        mtu: response.words[4] as u32,
        address: response.words[5] as u32,
        prefix_len: response.words[6] as u8,
        gateway: response.words[7] as u32,
        mac: unpack_mac(response.words[8]),
        rx_packets: response.words[9],
        tx_packets: response.words[10],
        dropped_packets: response.words[11],
    }))
}

pub fn network_resolve(
    network_handle: Handle,
    name: &str,
    addresses: &mut [u32],
) -> Result<usize> {
    let name_bytes = name.as_bytes();
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(1)) * 8;
    if name_bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let reply = channel_create()?;
    let mut request = RawMessage::empty(NetworkTag::ResolveRequest as u32);
    request.word_count = 1 + pack_bytes(name_bytes, &mut request.words[1..])?;
    request.words[0] = name_bytes.len() as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(network_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != NetworkTag::ResolveReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }

    let status = network_status_from_word(response.words[0]);
    if status != NetworkStatus::Ok {
        return Err(network_status_error(status));
    }

    let count = response.words[1] as usize;
    if count > addresses.len() || (response.word_count as usize) < 2 + count {
        return Err(Error::BufferTooSmall);
    }
    for (index, address) in addresses.iter_mut().enumerate().take(count) {
        *address = response.words[2 + index] as u32;
    }
    Ok(count)
}

pub fn network_ping(network_handle: Handle, target: &str) -> Result<(u32, u64)> {
    let target_bytes = target.as_bytes();
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(1)) * 8;
    if target_bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let reply = channel_create()?;
    let mut request = RawMessage::empty(NetworkTag::PingRequest as u32);
    request.word_count = 1 + pack_bytes(target_bytes, &mut request.words[1..])?;
    request.words[0] = target_bytes.len() as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(network_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != NetworkTag::PingReply as u32 || response.word_count < 3 {
        return Err(Error::InvalidArgument);
    }

    let status = network_status_from_word(response.words[0]);
    if status != NetworkStatus::Ok {
        return Err(network_status_error(status));
    }

    Ok((response.words[1] as u32, response.words[2]))
}

pub fn graphics_output_count(graphics_handle: Handle) -> Result<usize> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(GraphicsTag::OutputListRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(graphics_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != GraphicsTag::OutputListReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }

    match graphics_status_from_word(response.words[0]) {
        GraphicsStatus::Ok => Ok(response.words[1] as usize),
        status => Err(graphics_status_error(status)),
    }
}

pub fn graphics_output_status(
    graphics_handle: Handle,
    index: usize,
) -> Result<Option<GraphicsOutputStatusInfo>> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(GraphicsTag::OutputStatusRequest as u32);
    request.word_count = 1;
    request.words[0] = index as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(graphics_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != GraphicsTag::OutputStatusReply as u32 || response.word_count < 12 {
        return Err(Error::InvalidArgument);
    }

    let status = graphics_status_from_word(response.words[0]);
    if status == GraphicsStatus::NotFound {
        return Ok(None);
    }
    if status != GraphicsStatus::Ok {
        return Err(graphics_status_error(status));
    }

    Ok(Some(GraphicsOutputStatusInfo {
        index: response.words[1] as u32,
        backend: display_backend_from_word(response.words[2]),
        state: display_state_from_word(response.words[3]),
        pixel_format: display_pixel_format_from_word(response.words[4]),
        width: response.words[5] as u32,
        height: response.words[6] as u32,
        stride: response.words[7] as u32,
        bytes_per_pixel: response.words[8] as u32,
        byte_len: response.words[9],
        present_count: response.words[10],
        surface_count: response.words[11] as u32,
    }))
}

pub fn graphics_surface_create(
    graphics_handle: Handle,
    owner_session: u32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    z_order: u32,
    fill_rgb: u32,
    visible: bool,
) -> Result<(u32, Handle)> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(GraphicsTag::SurfaceCreateRequest as u32);
    request.word_count = 8;
    request.words[0] = owner_session as u64;
    request.words[1] = x as i64 as u64;
    request.words[2] = y as i64 as u64;
    request.words[3] = width as u64;
    request.words[4] = height as u64;
    request.words[5] = z_order as u64;
    request.words[6] = fill_rgb as u64;
    request.words[7] = u64::from(visible);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(graphics_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != GraphicsTag::SurfaceCreateReply as u32
        || response.word_count < 2
        || response.handle_count < 1
    {
        return Err(Error::InvalidArgument);
    }

    match graphics_status_from_word(response.words[0]) {
        GraphicsStatus::Ok => Ok((response.words[1] as u32, response.handles[0])),
        status => Err(graphics_status_error(status)),
    }
}

pub fn graphics_surface_list(graphics_handle: Handle, ids: &mut [u32]) -> Result<usize> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(GraphicsTag::SurfaceListRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(graphics_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != GraphicsTag::SurfaceListReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }

    let status = graphics_status_from_word(response.words[0]);
    if status != GraphicsStatus::Ok {
        return Err(graphics_status_error(status));
    }
    let count = response.words[1] as usize;
    if count > ids.len() || (response.word_count as usize) < 2 + count {
        return Err(Error::BufferTooSmall);
    }
    for (index, id) in ids.iter_mut().enumerate().take(count) {
        *id = response.words[2 + index] as u32;
    }
    Ok(count)
}

pub fn graphics_surface_status(
    graphics_handle: Handle,
    surface_id: u32,
) -> Result<Option<GraphicsSurfaceStatusInfo>> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(GraphicsTag::SurfaceStatusRequest as u32);
    request.word_count = 1;
    request.words[0] = surface_id as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(graphics_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != GraphicsTag::SurfaceStatusReply as u32 || response.word_count < 11 {
        return Err(Error::InvalidArgument);
    }

    let status = graphics_status_from_word(response.words[0]);
    if status == GraphicsStatus::NotFound {
        return Ok(None);
    }
    if status != GraphicsStatus::Ok {
        return Err(graphics_status_error(status));
    }

    Ok(Some(GraphicsSurfaceStatusInfo {
        surface_id: response.words[1] as u32,
        output_index: response.words[2] as u32,
        owner_session: response.words[3] as u32,
        x: response.words[4] as i64 as i32,
        y: response.words[5] as i64 as i32,
        width: response.words[6] as u32,
        height: response.words[7] as u32,
        z_order: response.words[8] as u32,
        fill_rgb: response.words[9] as u32,
        visible: response.words[10] != 0,
    }))
}

pub fn surface_set_geometry(
    surface_handle: Handle,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    z_order: u32,
) -> Result<()> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(SurfaceTag::SetGeometryRequest as u32);
    request.word_count = 5;
    request.words[0] = x as i64 as u64;
    request.words[1] = y as i64 as u64;
    request.words[2] = width as u64;
    request.words[3] = height as u64;
    request.words[4] = z_order as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(surface_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != SurfaceTag::SetGeometryReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match graphics_status_from_word(response.words[0]) {
        GraphicsStatus::Ok => Ok(()),
        status => Err(graphics_status_error(status)),
    }
}

pub fn surface_set_geometry_async(
    surface_handle: Handle,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    z_order: u32,
) -> Result<()> {
    let mut request = RawMessage::empty(SurfaceTag::SetGeometryRequest as u32);
    request.word_count = 5;
    request.words[0] = x as i64 as u64;
    request.words[1] = y as i64 as u64;
    request.words[2] = width as u64;
    request.words[3] = height as u64;
    request.words[4] = z_order as u64;
    channel_send(surface_handle, &request)
}

pub fn surface_set_fill(surface_handle: Handle, fill_rgb: u32) -> Result<()> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(SurfaceTag::SetFillRequest as u32);
    request.word_count = 1;
    request.words[0] = fill_rgb as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(surface_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != SurfaceTag::SetFillReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match graphics_status_from_word(response.words[0]) {
        GraphicsStatus::Ok => Ok(()),
        status => Err(graphics_status_error(status)),
    }
}

pub fn surface_set_visibility(surface_handle: Handle, visible: bool) -> Result<()> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(SurfaceTag::SetVisibilityRequest as u32);
    request.word_count = 1;
    request.words[0] = u64::from(visible);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(surface_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != SurfaceTag::SetVisibilityReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match graphics_status_from_word(response.words[0]) {
        GraphicsStatus::Ok => Ok(()),
        status => Err(graphics_status_error(status)),
    }
}

pub fn surface_clear_scene(surface_handle: Handle) -> Result<()> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(SurfaceTag::ClearSceneRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(surface_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != SurfaceTag::ClearSceneReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match graphics_status_from_word(response.words[0]) {
        GraphicsStatus::Ok => Ok(()),
        status => Err(graphics_status_error(status)),
    }
}

pub fn surface_set_rect(
    surface_handle: Handle,
    slot: u32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    color_rgb: u32,
    visible: bool,
) -> Result<()> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(SurfaceTag::SetRectRequest as u32);
    request.word_count = 7;
    request.words[0] = slot as u64;
    request.words[1] = x as i64 as u64;
    request.words[2] = y as i64 as u64;
    request.words[3] = width as u64;
    request.words[4] = height as u64;
    request.words[5] = color_rgb as u64;
    request.words[6] = u64::from(visible);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(surface_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != SurfaceTag::SetRectReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match graphics_status_from_word(response.words[0]) {
        GraphicsStatus::Ok => Ok(()),
        status => Err(graphics_status_error(status)),
    }
}

pub fn surface_set_label(
    surface_handle: Handle,
    slot: u32,
    x: i32,
    y: i32,
    color_rgb: u32,
    text: &str,
) -> Result<()> {
    let text_bytes = text.as_bytes();
    let packed_words = text_bytes.len().div_ceil(8);
    if 5 + packed_words > IPC_MAX_WORDS {
        return Err(Error::BufferTooSmall);
    }

    let reply = channel_create()?;
    let mut request = RawMessage::empty(SurfaceTag::SetLabelRequest as u32);
    request.word_count = 5 + pack_bytes(text_bytes, &mut request.words[5..])?;
    request.words[0] = slot as u64;
    request.words[1] = x as i64 as u64;
    request.words[2] = y as i64 as u64;
    request.words[3] = color_rgb as u64;
    request.words[4] = text_bytes.len() as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(surface_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != SurfaceTag::SetLabelReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match graphics_status_from_word(response.words[0]) {
        GraphicsStatus::Ok => Ok(()),
        status => Err(graphics_status_error(status)),
    }
}

pub fn surface_close(surface_handle: Handle) -> Result<()> {
    let request = RawMessage::empty(SurfaceTag::CloseRequest as u32);
    channel_send(surface_handle, &request).map(|_| ())
}

pub fn surface_attach_buffer(
    surface_handle: Handle,
    buffer_handle: Handle,
    width: u32,
    height: u32,
    stride_pixels: u32,
) -> Result<()> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(SurfaceTag::AttachBufferRequest as u32);
    request.word_count = 3;
    request.words[0] = width as u64;
    request.words[1] = height as u64;
    request.words[2] = stride_pixels as u64;
    request.handle_count = 2;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    request.handles[1] = buffer_handle;
    request.handle_rights[1] = rights::READ;
    channel_send(surface_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != SurfaceTag::AttachBufferReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    match graphics_status_from_word(response.words[0]) {
        GraphicsStatus::Ok => Ok(()),
        status => Err(graphics_status_error(status)),
    }
}

pub fn app_control_focus(control_handle: Handle, focused: bool) -> Result<()> {
    let mut request = RawMessage::empty(AppControlTag::FocusChanged as u32);
    request.word_count = 1;
    request.words[0] = u64::from(focused);
    channel_send(control_handle, &request)
}

pub fn app_control_resize(control_handle: Handle, width: u32, height: u32) -> Result<()> {
    let mut request = RawMessage::empty(AppControlTag::Resize as u32);
    request.word_count = 2;
    request.words[0] = width as u64;
    request.words[1] = height as u64;
    channel_send(control_handle, &request)
}

pub fn app_control_close(control_handle: Handle) -> Result<()> {
    let request = RawMessage::empty(AppControlTag::Close as u32);
    channel_send(control_handle, &request)
}

pub fn app_control_pointer(
    control_handle: Handle,
    action: AppPointerAction,
    x: i32,
    y: i32,
    button: u32,
) -> Result<()> {
    let mut request = RawMessage::empty(AppControlTag::Pointer as u32);
    request.word_count = 4;
    request.words[0] = action as u32 as u64;
    request.words[1] = x as i64 as u64;
    request.words[2] = y as i64 as u64;
    request.words[3] = button as u64;
    channel_send(control_handle, &request)
}

pub fn app_control_key(
    control_handle: Handle,
    action: AppKeyAction,
    key_code: u32,
) -> Result<()> {
    let mut request = RawMessage::empty(AppControlTag::Key as u32);
    request.word_count = 2;
    request.words[0] = action as u32 as u64;
    request.words[1] = key_code as u64;
    channel_send(control_handle, &request)
}

pub fn app_control_text(control_handle: Handle, scalar: char) -> Result<()> {
    let mut request = RawMessage::empty(AppControlTag::Text as u32);
    request.word_count = 1;
    request.words[0] = scalar as u32 as u64;
    channel_send(control_handle, &request)
}

pub fn session_list(session_handle: Handle, ids: &mut [u32]) -> Result<usize> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(SessionTag::ListRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(session_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != SessionTag::ListReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    let status = session_status_from_word(response.words[0]);
    if status != SessionStatus::Ok {
        return Err(session_status_error(status));
    }
    let count = response.words[1] as usize;
    if count > ids.len() || (response.word_count as usize) < 2 + count {
        return Err(Error::BufferTooSmall);
    }
    for (index, id) in ids.iter_mut().enumerate().take(count) {
        *id = response.words[2 + index] as u32;
    }
    Ok(count)
}

pub fn session_status(service_handle: Handle, session_id: u32) -> Result<Option<SessionStatusInfo>> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(SessionTag::StatusRequest as u32);
    request.word_count = 1;
    request.words[0] = session_id as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(service_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != SessionTag::StatusReply as u32 || response.word_count < 5 {
        return Err(Error::InvalidArgument);
    }
    let status = session_status_from_word(response.words[0]);
    if status == SessionStatus::NotFound {
        return Ok(None);
    }
    if status != SessionStatus::Ok {
        return Err(session_status_error(status));
    }

    Ok(Some(SessionStatusInfo {
        session_id: response.words[1] as u32,
        input_source: session_input_source_from_word(response.words[2]),
        focused_surface: response.words[3] as u32,
        surface_count: response.words[4] as u32,
    }))
}

pub fn session_focus(service_handle: Handle, session_id: u32, surface_id: u32) -> Result<u32> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(SessionTag::FocusRequest as u32);
    request.word_count = 2;
    request.words[0] = session_id as u64;
    request.words[1] = surface_id as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(service_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != SessionTag::FocusReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    match session_status_from_word(response.words[0]) {
        SessionStatus::Ok => Ok(response.words[1] as u32),
        status => Err(session_status_error(status)),
    }
}

pub fn storage_read_all(blob_handle: Handle, buffer: &mut [u8], expected_len: usize) -> Result<usize> {
    if expected_len > buffer.len() {
        return Err(Error::BufferTooSmall);
    }

    let mut offset = 0usize;
    while offset < expected_len {
        let read = storage_read(blob_handle, offset, &mut buffer[offset..expected_len])?;
        if read == 0 {
            break;
        }
        offset += read;
    }
    Ok(offset)
}

pub fn write_log(domain: &str, message: &str) -> Result<()> {
    let mut buffer = FixedLogBuffer::<192>::new();
    let _ = write!(&mut buffer, "{domain}: {message}");
    debug_log(buffer.as_bytes())
}

pub fn write_logf(domain: &str, args: fmt::Arguments<'_>) -> Result<()> {
    let mut buffer = FixedLogBuffer::<192>::new();
    let _ = write!(&mut buffer, "{domain}: ");
    let _ = buffer.write_fmt(args);
    debug_log(buffer.as_bytes())
}

pub struct FixedLogBuffer<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> FixedLogBuffer<N> {
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

impl<const N: usize> Write for FixedLogBuffer<N> {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let bytes = value.as_bytes();
        let remaining = N.saturating_sub(self.len);
        let copy_len = remaining.min(bytes.len());
        self.bytes[self.len..self.len + copy_len].copy_from_slice(&bytes[..copy_len]);
        self.len += copy_len;
        Ok(())
    }
}

#[macro_export]
macro_rules! entry {
    ($path:path) => {
        #[panic_handler]
        fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
            let _ = $crate::write_log("panic", "userspace panic");
            $crate::thread_exit(0xffff_ffff_ffff_ff00)
        }

        #[unsafe(no_mangle)]
        #[unsafe(link_section = ".text.start")]
        pub extern "C" fn _start() -> ! {
            let code: u64 = $path();
            $crate::thread_exit(code)
        }
    };
}

fn syscall0(number: SyscallNumber) -> Result<u64> {
    let (value, error) = raw_syscall(number as u32 as u64, 0, 0, 0, 0, 0, 0);
    decode_result(value, error)
}

fn syscall1(number: SyscallNumber, arg0: u64) -> Result<u64> {
    let (value, error) = raw_syscall(number as u32 as u64, arg0, 0, 0, 0, 0, 0);
    decode_result(value, error)
}

fn syscall2(number: SyscallNumber, arg0: u64, arg1: u64) -> Result<u64> {
    let (value, error) = raw_syscall(number as u32 as u64, arg0, arg1, 0, 0, 0, 0);
    decode_result(value, error)
}

fn syscall3(number: SyscallNumber, arg0: u64, arg1: u64, arg2: u64) -> Result<u64> {
    let (value, error) = raw_syscall(number as u32 as u64, arg0, arg1, arg2, 0, 0, 0);
    decode_result(value, error)
}

fn syscall4(number: SyscallNumber, arg0: u64, arg1: u64, arg2: u64, arg3: u64) -> Result<u64> {
    let (value, error) = raw_syscall(number as u32 as u64, arg0, arg1, arg2, arg3, 0, 0);
    decode_result(value, error)
}

fn pack_bytes(source: &[u8], words: &mut [u64]) -> Result<u32> {
    let required_words = source.len().div_ceil(8);
    if required_words > words.len() {
        return Err(Error::BufferTooSmall);
    }
    for (index, chunk) in source.chunks(8).enumerate() {
        let mut bytes = [0u8; 8];
        bytes[..chunk.len()].copy_from_slice(chunk);
        words[index] = u64::from_le_bytes(bytes);
    }
    Ok(required_words as u32)
}

fn unpack_bytes(words: &[u64], len: usize, destination: &mut [u8]) -> Result<()> {
    if len > destination.len() || len > words.len() * 8 {
        return Err(Error::BufferTooSmall);
    }

    let mut copied = 0usize;
    for word in words.iter().copied() {
        if copied >= len {
            break;
        }
        let bytes = word.to_le_bytes();
        let chunk = (len - copied).min(bytes.len());
        destination[copied..copied + chunk].copy_from_slice(&bytes[..chunk]);
        copied += chunk;
    }
    Ok(())
}

fn service_id_from_word(value: u64) -> ServiceId {
    match value as u32 {
        x if x == ServiceId::Storage as u32 => ServiceId::Storage,
        x if x == ServiceId::Console as u32 => ServiceId::Console,
        x if x == ServiceId::Config as u32 => ServiceId::Config,
        x if x == ServiceId::Log as u32 => ServiceId::Log,
        x if x == ServiceId::Status as u32 => ServiceId::Status,
        x if x == ServiceId::Shell as u32 => ServiceId::Shell,
        x if x == ServiceId::Package as u32 => ServiceId::Package,
        x if x == ServiceId::Announce as u32 => ServiceId::Announce,
        x if x == ServiceId::Network as u32 => ServiceId::Network,
        x if x == ServiceId::Graphics as u32 => ServiceId::Graphics,
        x if x == ServiceId::Session as u32 => ServiceId::Session,
        x if x == ServiceId::DesktopShell as u32 => ServiceId::DesktopShell,
        _ => ServiceId::RootManager,
    }
}

fn severity_from_word(value: u64) -> LogSeverity {
    match value as u32 {
        x if x == LogSeverity::Trace as u32 => LogSeverity::Trace,
        x if x == LogSeverity::Debug as u32 => LogSeverity::Debug,
        x if x == LogSeverity::Warn as u32 => LogSeverity::Warn,
        x if x == LogSeverity::Error as u32 => LogSeverity::Error,
        _ => LogSeverity::Info,
    }
}

fn domain_from_word(value: u64) -> LogDomain {
    match value as u32 {
        x if x == LogDomain::Bootstrap as u32 => LogDomain::Bootstrap,
        x if x == LogDomain::ServiceManager as u32 => LogDomain::ServiceManager,
        x if x == LogDomain::Storage as u32 => LogDomain::Storage,
        x if x == LogDomain::Log as u32 => LogDomain::Log,
        x if x == LogDomain::Config as u32 => LogDomain::Config,
        x if x == LogDomain::Console as u32 => LogDomain::Console,
        x if x == LogDomain::Status as u32 => LogDomain::Status,
        x if x == LogDomain::Ipc as u32 => LogDomain::Ipc,
        x if x == LogDomain::Shell as u32 => LogDomain::Shell,
        x if x == LogDomain::Package as u32 => LogDomain::Package,
        x if x == LogDomain::Network as u32 => LogDomain::Network,
        x if x == LogDomain::Graphics as u32 => LogDomain::Graphics,
        x if x == LogDomain::Session as u32 => LogDomain::Session,
        x if x == LogDomain::Desktop as u32 => LogDomain::Desktop,
        x if x == LogDomain::App as u32 => LogDomain::App,
        _ => LogDomain::Service,
    }
}

fn event_from_word(value: u64) -> LogEvent {
    match value as u32 {
        x if x == LogEvent::ServiceStarted as u32 => LogEvent::ServiceStarted,
        x if x == LogEvent::ServiceReady as u32 => LogEvent::ServiceReady,
        x if x == LogEvent::ServiceFailed as u32 => LogEvent::ServiceFailed,
        x if x == LogEvent::ServiceRestarting as u32 => LogEvent::ServiceRestarting,
        x if x == LogEvent::ConfigLoaded as u32 => LogEvent::ConfigLoaded,
        x if x == LogEvent::ConfigRead as u32 => LogEvent::ConfigRead,
        x if x == LogEvent::ConsoleWrite as u32 => LogEvent::ConsoleWrite,
        x if x == LogEvent::StatusStarted as u32 => LogEvent::StatusStarted,
        x if x == LogEvent::StatusHeartbeat as u32 => LogEvent::StatusHeartbeat,
        x if x == LogEvent::StorageMounted as u32 => LogEvent::StorageMounted,
        x if x == LogEvent::ManifestLoaded as u32 => LogEvent::ManifestLoaded,
        x if x == LogEvent::ResourceOpened as u32 => LogEvent::ResourceOpened,
        x if x == LogEvent::SessionOpened as u32 => LogEvent::SessionOpened,
        x if x == LogEvent::ShellCommand as u32 => LogEvent::ShellCommand,
        x if x == LogEvent::ToolLaunched as u32 => LogEvent::ToolLaunched,
        x if x == LogEvent::PackageCatalogLoaded as u32 => LogEvent::PackageCatalogLoaded,
        x if x == LogEvent::PackageInstalled as u32 => LogEvent::PackageInstalled,
        x if x == LogEvent::PackageUpdated as u32 => LogEvent::PackageUpdated,
        x if x == LogEvent::PackageRemoved as u32 => LogEvent::PackageRemoved,
        x if x == LogEvent::PackageRolledBack as u32 => LogEvent::PackageRolledBack,
        x if x == LogEvent::PackageActivationFailed as u32 => LogEvent::PackageActivationFailed,
        x if x == LogEvent::NetworkInterfaceReady as u32 => LogEvent::NetworkInterfaceReady,
        x if x == LogEvent::NetworkAddressConfigured as u32 => LogEvent::NetworkAddressConfigured,
        x if x == LogEvent::NetworkResolveCompleted as u32 => LogEvent::NetworkResolveCompleted,
        x if x == LogEvent::NetworkProbeCompleted as u32 => LogEvent::NetworkProbeCompleted,
        x if x == LogEvent::NetworkLinkChanged as u32 => LogEvent::NetworkLinkChanged,
        x if x == LogEvent::DisplayOutputReady as u32 => LogEvent::DisplayOutputReady,
        x if x == LogEvent::SurfaceCreated as u32 => LogEvent::SurfaceCreated,
        x if x == LogEvent::SurfaceUpdated as u32 => LogEvent::SurfaceUpdated,
        x if x == LogEvent::CompositorPresented as u32 => LogEvent::CompositorPresented,
        x if x == LogEvent::SessionReady as u32 => LogEvent::SessionReady,
        x if x == LogEvent::SessionFocusChanged as u32 => LogEvent::SessionFocusChanged,
        x if x == LogEvent::DesktopReady as u32 => LogEvent::DesktopReady,
        x if x == LogEvent::DesktopAppLaunched as u32 => LogEvent::DesktopAppLaunched,
        x if x == LogEvent::DesktopAppExited as u32 => LogEvent::DesktopAppExited,
        x if x == LogEvent::DesktopFocusChanged as u32 => LogEvent::DesktopFocusChanged,
        x if x == LogEvent::AppRendered as u32 => LogEvent::AppRendered,
        _ => LogEvent::LookupGranted,
    }
}

fn manager_phase_from_word(value: u64) -> ManagerServicePhase {
    match value as u32 {
        x if x == ManagerServicePhase::Dormant as u32 => ManagerServicePhase::Dormant,
        x if x == ManagerServicePhase::Starting as u32 => ManagerServicePhase::Starting,
        x if x == ManagerServicePhase::Exited as u32 => ManagerServicePhase::Exited,
        _ => ManagerServicePhase::Ready,
    }
}

fn manager_status_from_word(value: u64) -> ManagerStatus {
    match value as u32 {
        x if x == ManagerStatus::Ok as u32 => ManagerStatus::Ok,
        x if x == ManagerStatus::Denied as u32 => ManagerStatus::Denied,
        x if x == ManagerStatus::NotFound as u32 => ManagerStatus::NotFound,
        x if x == ManagerStatus::Busy as u32 => ManagerStatus::Busy,
        x if x == ManagerStatus::Failed as u32 => ManagerStatus::Failed,
        _ => ManagerStatus::Busy,
    }
}

fn package_status_from_word(value: u64) -> PackageStatus {
    match value as u32 {
        x if x == PackageStatus::Ok as u32 => PackageStatus::Ok,
        x if x == PackageStatus::NotFound as u32 => PackageStatus::NotFound,
        x if x == PackageStatus::AlreadyInstalled as u32 => PackageStatus::AlreadyInstalled,
        x if x == PackageStatus::NotInstalled as u32 => PackageStatus::NotInstalled,
        x if x == PackageStatus::Busy as u32 => PackageStatus::Busy,
        x if x == PackageStatus::Denied as u32 => PackageStatus::Denied,
        x if x == PackageStatus::IntegrityFailed as u32 => PackageStatus::IntegrityFailed,
        x if x == PackageStatus::End as u32 => PackageStatus::End,
        x if x == PackageStatus::NoChange as u32 => PackageStatus::NoChange,
        x if x == PackageStatus::NoRollback as u32 => PackageStatus::NoRollback,
        _ => PackageStatus::Busy,
    }
}

fn package_status_error(status: PackageStatus) -> Error {
    match status {
        PackageStatus::NotFound => Error::NotFound,
        PackageStatus::AlreadyInstalled | PackageStatus::Busy | PackageStatus::NoChange => Error::Busy,
        PackageStatus::NotInstalled | PackageStatus::NoRollback | PackageStatus::End => {
            Error::InvalidArgument
        }
        PackageStatus::Denied => Error::PermissionDenied,
        PackageStatus::IntegrityFailed => Error::InvalidCall,
        PackageStatus::Ok => Error::InvalidArgument,
    }
}

fn network_status_from_word(value: u64) -> NetworkStatus {
    match value as u32 {
        x if x == NetworkStatus::Ok as u32 => NetworkStatus::Ok,
        x if x == NetworkStatus::NotFound as u32 => NetworkStatus::NotFound,
        x if x == NetworkStatus::Busy as u32 => NetworkStatus::Busy,
        x if x == NetworkStatus::InvalidTarget as u32 => NetworkStatus::InvalidTarget,
        x if x == NetworkStatus::Timeout as u32 => NetworkStatus::Timeout,
        x if x == NetworkStatus::End as u32 => NetworkStatus::End,
        x if x == NetworkStatus::Unsupported as u32 => NetworkStatus::Unsupported,
        _ => NetworkStatus::Busy,
    }
}

fn network_status_error(status: NetworkStatus) -> Error {
    match status {
        NetworkStatus::Ok => Error::InvalidArgument,
        NetworkStatus::NotFound | NetworkStatus::InvalidTarget => Error::NotFound,
        NetworkStatus::Busy => Error::Busy,
        NetworkStatus::Timeout => Error::QueueEmpty,
        NetworkStatus::End => Error::NotFound,
        NetworkStatus::Unsupported => Error::Unsupported,
    }
}

fn graphics_status_from_word(value: u64) -> GraphicsStatus {
    match value as u32 {
        x if x == GraphicsStatus::Ok as u32 => GraphicsStatus::Ok,
        x if x == GraphicsStatus::NotFound as u32 => GraphicsStatus::NotFound,
        x if x == GraphicsStatus::Busy as u32 => GraphicsStatus::Busy,
        x if x == GraphicsStatus::Denied as u32 => GraphicsStatus::Denied,
        x if x == GraphicsStatus::CapacityExceeded as u32 => GraphicsStatus::CapacityExceeded,
        _ => GraphicsStatus::Busy,
    }
}

fn graphics_status_error(status: GraphicsStatus) -> Error {
    match status {
        GraphicsStatus::Ok => Error::InvalidArgument,
        GraphicsStatus::NotFound => Error::NotFound,
        GraphicsStatus::Busy => Error::Busy,
        GraphicsStatus::Denied => Error::PermissionDenied,
        GraphicsStatus::CapacityExceeded => Error::CapacityExceeded,
    }
}

fn session_status_from_word(value: u64) -> SessionStatus {
    match value as u32 {
        x if x == SessionStatus::Ok as u32 => SessionStatus::Ok,
        x if x == SessionStatus::NotFound as u32 => SessionStatus::NotFound,
        x if x == SessionStatus::Busy as u32 => SessionStatus::Busy,
        x if x == SessionStatus::Denied as u32 => SessionStatus::Denied,
        _ => SessionStatus::Busy,
    }
}

fn session_status_error(status: SessionStatus) -> Error {
    match status {
        SessionStatus::Ok => Error::InvalidArgument,
        SessionStatus::NotFound => Error::NotFound,
        SessionStatus::Busy => Error::Busy,
        SessionStatus::Denied => Error::PermissionDenied,
    }
}

fn desktop_status_from_word(value: u64) -> DesktopStatus {
    match value as u32 {
        x if x == DesktopStatus::Ok as u32 => DesktopStatus::Ok,
        x if x == DesktopStatus::NotFound as u32 => DesktopStatus::NotFound,
        x if x == DesktopStatus::Busy as u32 => DesktopStatus::Busy,
        x if x == DesktopStatus::Denied as u32 => DesktopStatus::Denied,
        _ => DesktopStatus::Busy,
    }
}

fn desktop_status_error(status: DesktopStatus) -> Error {
    match status {
        DesktopStatus::Ok => Error::InvalidArgument,
        DesktopStatus::NotFound => Error::NotFound,
        DesktopStatus::Busy => Error::Busy,
        DesktopStatus::Denied => Error::PermissionDenied,
    }
}

fn desktop_app_id_from_word(value: u64) -> core::result::Result<DesktopAppId, ()> {
    match value as u32 {
        x if x == DesktopAppId::Settings as u32 => Ok(DesktopAppId::Settings),
        x if x == DesktopAppId::Files as u32 => Ok(DesktopAppId::Files),
        x if x == DesktopAppId::Monitor as u32 => Ok(DesktopAppId::Monitor),
        _ => Err(()),
    }
}

fn desktop_drag_mode_from_word(value: u64) -> DesktopDragMode {
    match value as u32 {
        x if x == DesktopDragMode::Move as u32 => DesktopDragMode::Move,
        x if x == DesktopDragMode::Resize as u32 => DesktopDragMode::Resize,
        _ => DesktopDragMode::None,
    }
}

fn unpack_i32_pair(value: u64) -> (i32, i32) {
    (value as u32 as i32, (value >> 32) as u32 as i32)
}

fn unpack_u32_pair(value: u64) -> (u32, u32) {
    (value as u32, (value >> 32) as u32)
}

fn display_backend_from_word(value: u64) -> DisplayOutputBackend {
    match value as u32 {
        x if x == DisplayOutputBackend::BootFramebuffer as u32 => {
            DisplayOutputBackend::BootFramebuffer
        }
        _ => DisplayOutputBackend::Unknown,
    }
}

fn display_state_from_word(value: u64) -> DisplayOutputState {
    match value as u32 {
        x if x == DisplayOutputState::Connected as u32 => DisplayOutputState::Connected,
        _ => DisplayOutputState::Disconnected,
    }
}

fn display_pixel_format_from_word(value: u64) -> DisplayPixelFormat {
    match value as u32 {
        x if x == DisplayPixelFormat::Xrgb8888 as u32 => DisplayPixelFormat::Xrgb8888,
        x if x == DisplayPixelFormat::Bgrx8888 as u32 => DisplayPixelFormat::Bgrx8888,
        _ => DisplayPixelFormat::Unknown,
    }
}

fn session_input_source_from_word(value: u64) -> SessionInputSource {
    match value as u32 {
        x if x == SessionInputSource::ServiceControl as u32 => SessionInputSource::ServiceControl,
        _ => SessionInputSource::None,
    }
}

fn packet_backend_from_word(value: u64) -> PacketInterfaceBackend {
    match value as u32 {
        x if x == PacketInterfaceBackend::VirtioPci as u32 => PacketInterfaceBackend::VirtioPci,
        _ => PacketInterfaceBackend::Unknown,
    }
}

fn packet_link_state_from_word(value: u64) -> PacketInterfaceLinkState {
    match value as u32 {
        x if x == PacketInterfaceLinkState::Up as u32 => PacketInterfaceLinkState::Up,
        _ => PacketInterfaceLinkState::Down,
    }
}

fn unpack_mac(word: u64) -> [u8; 6] {
    [
        (word & 0xff) as u8,
        ((word >> 8) & 0xff) as u8,
        ((word >> 16) & 0xff) as u8,
        ((word >> 24) & 0xff) as u8,
        ((word >> 32) & 0xff) as u8,
        ((word >> 40) & 0xff) as u8,
    ]
}

fn decode_result(value: u64, error: u64) -> Result<u64> {
    if error == 0 {
        Ok(value)
    } else {
        Err(Error::from_code(error))
    }
}

fn raw_syscall(
    number: u64,
    arg0: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    arg4: u64,
    arg5: u64,
) -> (u64, u64) {
    let mut value = number;
    let mut arg2_inout = arg2;

    unsafe {
        asm!(
            "int 0x80",
            inlateout("rax") value,
            in("rdi") arg0,
            in("rsi") arg1,
            inlateout("rdx") arg2_inout,
            in("r10") arg3,
            in("r8") arg4,
            in("r9") arg5,
        );
    }

    (value, arg2_inout)
}
