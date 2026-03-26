use core::sync::atomic::{AtomicU64, Ordering};

use serviceos_abi::{
    Handle, HandlePair, IPC_FLAG_NONBLOCK, IPC_MAX_HANDLES, IPC_MAX_WORDS, RawMessage,
    SyscallErrorCode as AbiErrorCode, SyscallNumber as AbiSyscallNumber, TaskStateCode,
    TaskStatus as AbiTaskStatus,
};
use spin::Once;

use crate::{
    capability::{
        CapabilityError, CapabilityHandle, CapabilityResolver, CapabilityRights, TransferMode,
    },
    ipc::{self, IpcError, MessageTag, OutgoingMessage},
    object::ObjectId,
    task::TaskRole,
    time,
    user::{self, AddressSpacePreparationError, LoadError, SpawnError},
};

const SYSCALL_ABI_VERSION: u64 = 0x0003_0000;
const MAX_SYSCALL_SLOTS: usize = 15;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyscallNumber(pub u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyscallContext {
    pub instruction_pointer: u64,
    pub stack_pointer: u64,
    pub flags: u64,
    pub arguments: [u64; 6],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyscallReturn {
    pub value: u64,
    pub error: Option<SyscallError>,
    pub action: SyscallAction,
}

impl SyscallReturn {
    pub const fn success(value: u64) -> Self {
        Self {
            value,
            error: None,
            action: SyscallAction::ReturnToCaller,
        }
    }

    pub const fn error(error: SyscallError) -> Self {
        Self {
            value: 0,
            error: Some(error),
            action: SyscallAction::ReturnToCaller,
        }
    }

    pub const fn error_with_action(error: SyscallError, action: SyscallAction) -> Self {
        Self {
            value: 0,
            error: Some(error),
            action,
        }
    }

    pub const fn action(value: u64, action: SyscallAction) -> Self {
        Self {
            value,
            error: None,
            action,
        }
    }

    pub const fn exit_current_thread(status: u64) -> Self {
        Self {
            value: status,
            error: None,
            action: SyscallAction::ExitCurrentThread { status },
        }
    }

    pub const fn abi_error_code(self) -> u64 {
        match self.error {
            None => 0,
            Some(error) => error.abi_code(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyscallAction {
    ReturnToCaller,
    YieldCurrentThread,
    BlockCurrentThreadOnReceive { endpoint: ObjectId },
    ExitCurrentThread { status: u64 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyscallError {
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
}

impl SyscallError {
    pub const fn abi_code(self) -> u64 {
        match self {
            Self::Unsupported => AbiErrorCode::Unsupported as u64,
            Self::InvalidCall => AbiErrorCode::InvalidCall as u64,
            Self::PermissionDenied => AbiErrorCode::PermissionDenied as u64,
            Self::NotInitialized => AbiErrorCode::NotInitialized as u64,
            Self::InvalidArgument => AbiErrorCode::InvalidArgument as u64,
            Self::BufferTooSmall => AbiErrorCode::BufferTooSmall as u64,
            Self::QueueEmpty => AbiErrorCode::QueueEmpty as u64,
            Self::NotFound => AbiErrorCode::NotFound as u64,
            Self::Busy => AbiErrorCode::Busy as u64,
            Self::CapacityExceeded => AbiErrorCode::CapacityExceeded as u64,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyscallKind {
    AbiVersion = AbiSyscallNumber::AbiVersion as isize,
    MonotonicNow = AbiSyscallNumber::MonotonicNow as isize,
    ThreadExit = AbiSyscallNumber::ThreadExit as isize,
    YieldCurrent = AbiSyscallNumber::YieldCurrent as isize,
    DebugLogWrite = AbiSyscallNumber::DebugLogWrite as isize,
    ChannelCreate = AbiSyscallNumber::ChannelCreate as isize,
    ChannelSend = AbiSyscallNumber::ChannelSend as isize,
    ChannelReceive = AbiSyscallNumber::ChannelReceive as isize,
    HandleDuplicate = AbiSyscallNumber::HandleDuplicate as isize,
    HandleClose = AbiSyscallNumber::HandleClose as isize,
    ServiceSpawn = AbiSyscallNumber::ServiceSpawn as isize,
    TaskStatus = AbiSyscallNumber::TaskStatus as isize,
    MemoryRead = AbiSyscallNumber::MemoryRead as isize,
    DebugConsoleRead = AbiSyscallNumber::DebugConsoleRead as isize,
    DebugConsoleWrite = AbiSyscallNumber::DebugConsoleWrite as isize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyscallSnapshot {
    pub dispatched: u64,
    pub rejected: u64,
}

pub trait SyscallDispatcher {
    fn dispatch(&self, number: SyscallNumber, context: &SyscallContext) -> SyscallReturn;
}

type Handler = fn(&SyscallContext) -> SyscallReturn;

pub struct DispatchTable {
    entries: [Option<Handler>; MAX_SYSCALL_SLOTS],
    dispatched: AtomicU64,
    rejected: AtomicU64,
}

impl DispatchTable {
    const fn new(entries: [Option<Handler>; MAX_SYSCALL_SLOTS]) -> Self {
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
static DEBUG_LOG_WRITER: Once<fn(&[u8])> = Once::new();
static DEBUG_CONSOLE_READER: Once<fn() -> Option<u8>> = Once::new();
static DEBUG_CONSOLE_WRITER: Once<fn(&[u8])> = Once::new();

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
        ])
    })
}

pub fn dispatcher() -> Option<&'static DispatchTable> {
    DISPATCHER.get()
}

pub fn register_debug_log_writer(writer: fn(&[u8])) {
    let _ = DEBUG_LOG_WRITER.call_once(|| writer);
}

pub fn register_debug_console_reader(reader: fn() -> Option<u8>) {
    let _ = DEBUG_CONSOLE_READER.call_once(|| reader);
}

pub fn register_debug_console_writer(writer: fn(&[u8])) {
    let _ = DEBUG_CONSOLE_WRITER.call_once(|| writer);
}

fn handle_abi_version(_context: &SyscallContext) -> SyscallReturn {
    SyscallReturn::success(SYSCALL_ABI_VERSION)
}

fn handle_monotonic_now(_context: &SyscallContext) -> SyscallReturn {
    match time::manager() {
        Some(manager) => SyscallReturn::success(manager.now().0),
        None => SyscallReturn::error(SyscallError::NotInitialized),
    }
}

fn handle_thread_exit(context: &SyscallContext) -> SyscallReturn {
    SyscallReturn::exit_current_thread(context.arguments[0])
}

fn handle_yield_current(_context: &SyscallContext) -> SyscallReturn {
    SyscallReturn::action(0, SyscallAction::YieldCurrentThread)
}

fn handle_debug_log_write(context: &SyscallContext) -> SyscallReturn {
    let Some(writer) = DEBUG_LOG_WRITER.get().copied() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Ok(length) = usize::try_from(context.arguments[1]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(bytes) = (unsafe { user_slice(context.arguments[0], length) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    writer(bytes);
    SyscallReturn::success(length as u64)
}

fn handle_channel_create(context: &SyscallContext) -> SyscallReturn {
    let Some(current_task) = user::current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(task) = current_task.task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(ipc) = ipc::kernel() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(objects) = crate::object::model() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };

    let (first, second) = ipc.create_channel_pair(objects);
    let first_handle =
        match task
            .capability_space()
            .install(first, CapabilityRights::channel_endpoint(), None)
        {
            Ok(handle) => handle,
            Err(error) => return SyscallReturn::error(map_capability_error(error)),
        };
    let second_handle =
        match task
            .capability_space()
            .install(second, CapabilityRights::channel_endpoint(), None)
        {
            Ok(handle) => handle,
            Err(error) => return SyscallReturn::error(map_capability_error(error)),
        };
    let Ok(pair_out) = (unsafe { user_mut::<HandlePair>(context.arguments[0]) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    *pair_out = HandlePair {
        first: first_handle.0,
        second: second_handle.0,
    };
    SyscallReturn::success(0)
}

fn handle_channel_send(context: &SyscallContext) -> SyscallReturn {
    let Some(current_task) = user::current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(task) = current_task.task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(ipc) = ipc::kernel() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Ok(raw) = (unsafe { user_ref::<RawMessage>(context.arguments[1]) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    let word_count = raw.word_count as usize;
    let handle_count = raw.handle_count as usize;
    if word_count > IPC_MAX_WORDS || handle_count > IPC_MAX_HANDLES {
        return SyscallReturn::error(SyscallError::BufferTooSmall);
    }

    let mut message = match OutgoingMessage::new(MessageTag(raw.tag), &raw.words[..word_count]) {
        Ok(message) => message,
        Err(error) => return SyscallReturn::error(map_ipc_error(error)),
    };
    for (index, handle) in raw.handles[..handle_count].iter().copied().enumerate() {
        let Some(descriptor) = task
            .capability_space()
            .resolve_descriptor(CapabilityHandle(handle))
        else {
            return SyscallReturn::error(SyscallError::NotFound);
        };
        let requested_bits = raw.handle_rights[index];
        let transfer_rights = if requested_bits == 0 {
            descriptor
                .rights
                .without(CapabilityRights::DUPLICATE.union(CapabilityRights::TRANSFER))
        } else {
            CapabilityRights::from_bits(requested_bits)
        };
        let transfer = match task.capability_space().prepare_transfer(
            CapabilityHandle(handle),
            transfer_rights,
            TransferMode::Copy,
        ) {
            Ok(transfer) => transfer,
            Err(error) => return SyscallReturn::error(map_capability_error(error)),
        };
        message = match message.add_transfer(transfer) {
            Ok(message) => message,
            Err(error) => return SyscallReturn::error(map_ipc_error(error)),
        };
    }

    match ipc.send(
        task.capability_space(),
        CapabilityHandle(context.arguments[0] as Handle),
        message,
    ) {
        Ok(_) => SyscallReturn::success(0),
        Err(error) => SyscallReturn::error(map_ipc_error(error)),
    }
}

fn handle_channel_receive(context: &SyscallContext) -> SyscallReturn {
    let Some(current_task) = user::current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(task) = current_task.task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(ipc_kernel) = ipc::kernel() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Ok(message_out) = (unsafe { user_mut::<RawMessage>(context.arguments[1]) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    match ipc_kernel.receive(
        task.capability_space(),
        CapabilityHandle(context.arguments[0] as Handle),
    ) {
        Ok(message) => {
            if message.words.len() > IPC_MAX_WORDS
                || message.transferred_capabilities.len() > IPC_MAX_HANDLES
            {
                return SyscallReturn::error(SyscallError::BufferTooSmall);
            }

            let mut raw = RawMessage::empty(message.tag.0);
            raw.word_count = message.words.len() as u32;
            raw.handle_count = message.transferred_capabilities.len() as u32;
            raw.flags = message_out.flags;
            for (index, word) in message.words.iter().copied().enumerate() {
                raw.words[index] = word;
            }
            for (index, handle) in message.transferred_capabilities.iter().copied().enumerate() {
                raw.handles[index] = handle.0;
            }
            *message_out = raw;
            SyscallReturn::success(message.tag.0 as u64)
        }
        Err(IpcError::QueueEmpty) if message_out.flags & IPC_FLAG_NONBLOCK != 0 => {
            SyscallReturn::error(SyscallError::QueueEmpty)
        }
        Err(IpcError::QueueEmpty) => {
            let endpoint = match ipc_kernel.endpoint_object_id(
                task.capability_space(),
                CapabilityHandle(context.arguments[0] as Handle),
                CapabilityRights::RECEIVE,
            ) {
                Ok(endpoint) => endpoint,
                Err(error) => return SyscallReturn::error(map_ipc_error(error)),
            };
            SyscallReturn::error_with_action(
                SyscallError::QueueEmpty,
                SyscallAction::BlockCurrentThreadOnReceive { endpoint },
            )
        }
        Err(error) => SyscallReturn::error(map_ipc_error(error)),
    }
}

fn handle_handle_duplicate(context: &SyscallContext) -> SyscallReturn {
    let Some(current_task) = user::current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(task) = current_task.task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let rights = CapabilityRights::from_bits(context.arguments[1]);
    match task.capability_space().duplicate(
        CapabilityHandle(context.arguments[0] as Handle),
        rights,
        None,
    ) {
        Ok(handle) => SyscallReturn::success(handle.0 as u64),
        Err(error) => SyscallReturn::error(map_capability_error(error)),
    }
}

fn handle_handle_close(context: &SyscallContext) -> SyscallReturn {
    let Some(current_task) = user::current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(task) = current_task.task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    match task
        .capability_space()
        .close(CapabilityHandle(context.arguments[0] as Handle))
    {
        Ok(_) => SyscallReturn::success(0),
        Err(error) => SyscallReturn::error(map_capability_error(error)),
    }
}

fn handle_service_spawn(context: &SyscallContext) -> SyscallReturn {
    let Some(current_task) = user::current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(task) = current_task.task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let authority = match task.capability_space().resolve(
        CapabilityHandle(context.arguments[1] as Handle),
        CapabilityRights::bootstrap(),
    ) {
        Ok(authority) => authority,
        Err(error) => return SyscallReturn::error(map_capability_error(error)),
    };
    if authority.object.bootstrap_capability().is_none() {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    }

    let bootstrap_transfer = if context.arguments[2] == 0 {
        None
    } else {
        let handle = CapabilityHandle(context.arguments[2] as Handle);
        let Some(descriptor) = task.capability_space().resolve_descriptor(handle) else {
            return SyscallReturn::error(SyscallError::NotFound);
        };
        match task.capability_space().prepare_transfer(
            handle,
            descriptor.rights,
            TransferMode::Move,
        ) {
            Ok(transfer) => Some(transfer),
            Err(error) => return SyscallReturn::error(map_capability_error(error)),
        }
    };

    let spawned = match user::spawn_builtin_task(
        context.arguments[0] as u32,
        TaskRole::SystemService,
        bootstrap_transfer,
    ) {
        Ok(spawned) => spawned,
        Err(SpawnError::ImageNotFound) => return SyscallReturn::error(SyscallError::NotFound),
        Err(SpawnError::Capability(error)) => {
            return SyscallReturn::error(map_capability_error(error));
        }
        Err(SpawnError::Scheduler(_)) => return SyscallReturn::error(SyscallError::Busy),
        Err(SpawnError::AddressSpace(AddressSpacePreparationError::Load(
            LoadError::FrameExhausted,
        )))
        | Err(SpawnError::AddressSpace(AddressSpacePreparationError::Mapping(
            crate::memory::MappingError::FrameAllocationFailed,
        ))) => {
            return SyscallReturn::error(SyscallError::CapacityExceeded);
        }
        Err(_) => return SyscallReturn::error(SyscallError::NotInitialized),
    };

    match task
        .capability_space()
        .install(spawned.task, CapabilityRights::task(), None)
    {
        Ok(handle) => SyscallReturn::success(handle.0 as u64),
        Err(error) => SyscallReturn::error(map_capability_error(error)),
    }
}

fn handle_task_status(context: &SyscallContext) -> SyscallReturn {
    let Some(current_task) = user::current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(task) = current_task.task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(descriptor) = task
        .capability_space()
        .resolve_descriptor(CapabilityHandle(context.arguments[0] as Handle))
    else {
        return SyscallReturn::error(SyscallError::NotFound);
    };
    let Some(object) =
        crate::object::model().and_then(|model| model.registry().lookup(descriptor.object))
    else {
        return SyscallReturn::error(SyscallError::NotFound);
    };
    let Some(target_task) = object.task() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(status_out) = (unsafe { user_mut::<AbiTaskStatus>(context.arguments[1]) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    *status_out = match target_task.exit_status() {
        user::TaskExitStatus::Running => AbiTaskStatus {
            state: TaskStateCode::Running,
            exit_code: 0,
        },
        user::TaskExitStatus::Exited { code } => AbiTaskStatus {
            state: TaskStateCode::Exited,
            exit_code: code,
        },
    };

    SyscallReturn::success(0)
}

fn handle_memory_read(context: &SyscallContext) -> SyscallReturn {
    let Some(current_task) = user::current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(task) = current_task.task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(descriptor) = task
        .capability_space()
        .resolve_descriptor(CapabilityHandle(context.arguments[0] as Handle))
    else {
        return SyscallReturn::error(SyscallError::NotFound);
    };
    if !descriptor.rights.contains(CapabilityRights::READ) {
        return SyscallReturn::error(SyscallError::PermissionDenied);
    }
    let Some(object) =
        crate::object::model().and_then(|model| model.registry().lookup(descriptor.object))
    else {
        return SyscallReturn::error(SyscallError::NotFound);
    };
    let Some(memory) = object.memory_object() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(offset) = usize::try_from(context.arguments[1]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(length) = usize::try_from(context.arguments[3]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(destination) = (unsafe { user_slice_mut(context.arguments[2], length) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    SyscallReturn::success(memory.read(offset, destination) as u64)
}

fn handle_debug_console_read(_context: &SyscallContext) -> SyscallReturn {
    let Some(reader) = DEBUG_CONSOLE_READER.get().copied() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };

    match reader() {
        Some(byte) => SyscallReturn::success(byte as u64),
        None => SyscallReturn::error(SyscallError::QueueEmpty),
    }
}

fn handle_debug_console_write(context: &SyscallContext) -> SyscallReturn {
    let Some(writer) = DEBUG_CONSOLE_WRITER.get().copied() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Ok(length) = usize::try_from(context.arguments[1]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(bytes) = (unsafe { user_slice(context.arguments[0], length) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    writer(bytes);
    SyscallReturn::success(length as u64)
}

fn map_capability_error(error: CapabilityError) -> SyscallError {
    match error {
        CapabilityError::InvalidHandle => SyscallError::NotFound,
        CapabilityError::HandleSpaceExhausted => SyscallError::CapacityExceeded,
        CapabilityError::RightsViolation { .. }
        | CapabilityError::DuplicateForbidden
        | CapabilityError::TransferForbidden
        | CapabilityError::RequestedRightsExceedSource => SyscallError::PermissionDenied,
    }
}

fn map_ipc_error(error: IpcError) -> SyscallError {
    match error {
        IpcError::Capability(error) => map_capability_error(error),
        IpcError::EndpointNotReady | IpcError::EndpointClosed => SyscallError::Busy,
        IpcError::BufferShapeInvalid
        | IpcError::ObjectKindMismatch
        | IpcError::InvalidReplyEndpoint => SyscallError::InvalidArgument,
        IpcError::QueueEmpty => SyscallError::QueueEmpty,
        IpcError::QueueFull { .. }
        | IpcError::MessageTooLarge { .. }
        | IpcError::TooManyTransfers { .. } => SyscallError::CapacityExceeded,
    }
}

unsafe fn user_ref<T>(address: u64) -> Result<&'static T, SyscallError> {
    if address == 0 || address as usize % core::mem::align_of::<T>() != 0 {
        return Err(SyscallError::InvalidArgument);
    }

    Ok(unsafe { &*(address as *const T) })
}

unsafe fn user_mut<T>(address: u64) -> Result<&'static mut T, SyscallError> {
    if address == 0 || address as usize % core::mem::align_of::<T>() != 0 {
        return Err(SyscallError::InvalidArgument);
    }

    Ok(unsafe { &mut *(address as *mut T) })
}

unsafe fn user_slice(address: u64, len: usize) -> Result<&'static [u8], SyscallError> {
    if address == 0 {
        return Err(SyscallError::InvalidArgument);
    }

    Ok(unsafe { core::slice::from_raw_parts(address as *const u8, len) })
}

unsafe fn user_slice_mut(address: u64, len: usize) -> Result<&'static mut [u8], SyscallError> {
    if address == 0 {
        return Err(SyscallError::InvalidArgument);
    }

    Ok(unsafe { core::slice::from_raw_parts_mut(address as *mut u8, len) })
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
        let table = DispatchTable::new([
            Some(handle_abi_version),
            None,
            Some(handle_thread_exit),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        ]);

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
        let table = DispatchTable::new([
            Some(handle_abi_version),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        ]);

        let result = table.dispatch(
            SyscallNumber(SyscallKind::AbiVersion as u32),
            &empty_context(),
        );
        assert_eq!(result, SyscallReturn::success(SYSCALL_ABI_VERSION));
    }
}
