#![no_std]

use core::{
    arch::asm,
    fmt::{self, Write},
};

pub use serviceos_abi::{
    ConfigKey, ConfigTag, ConfigValueKind, ConsoleTag, ControlTag, Handle, HandlePair,
    IPC_FLAG_NONBLOCK, IPC_MAX_HANDLES, IPC_MAX_WORDS, INVALID_HANDLE, LifecycleEvent,
    LogDomain, LogEvent, LogQueryStatus, LogSeverity, LogTag, LookupStatus, ManagerAction,
    ManagerServicePhase, ManagerStatus, ManagerTag, PackageStatus, PackageTag, RawMessage,
    ServiceId, ServiceImageId, StatusTag, StorageStatus, StorageTag, SyscallErrorCode,
    SyscallNumber, TaskStateCode, TaskStatus,
};
pub use serviceos_abi::rights;

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
    if text_bytes.len() > IPC_MAX_WORDS * 8 {
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
    let mut request = RawMessage::empty(ManagerTag::LaunchRequest as u32);
    request.word_count = 1;
    request.words[0] = image_id as u32 as u64;
    if let Some(io_handle) = io_handle {
        request.handle_count = 1;
        request.handles[0] = io_handle;
        request.handle_rights[0] =
            rights::SEND | rights::RECEIVE | rights::DUPLICATE | rights::TRANSFER;
    }
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
