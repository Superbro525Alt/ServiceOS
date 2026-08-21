use serviceos_abi::{SyscallErrorCode as AbiErrorCode, SyscallNumber as AbiSyscallNumber};

use crate::object::ObjectId;

pub const SYSCALL_ABI_VERSION: u64 = 0x0003_0001;
pub const MAX_SYSCALL_SLOTS: usize = 48;

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
    BlockCurrentThreadOnPacketReceive { interface: ObjectId },
    BlockCurrentThreadOnInputReceive { source: ObjectId },
    BlockCurrentThreadOnObject { object: ObjectId },
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
    PacketInterfaceInfo = AbiSyscallNumber::PacketInterfaceInfo as isize,
    PacketInterfaceReceive = AbiSyscallNumber::PacketInterfaceReceive as isize,
    PacketInterfaceTransmit = AbiSyscallNumber::PacketInterfaceTransmit as isize,
    DisplayOutputInfo = AbiSyscallNumber::DisplayOutputInfo as isize,
    DisplayOutputPresent = AbiSyscallNumber::DisplayOutputPresent as isize,
    InputSourceInfo = AbiSyscallNumber::InputSourceInfo as isize,
    InputSourceReceive = AbiSyscallNumber::InputSourceReceive as isize,
    MemoryCreate = AbiSyscallNumber::MemoryCreate as isize,
    MemoryWrite = AbiSyscallNumber::MemoryWrite as isize,
    AudioEndpointInfo = AbiSyscallNumber::AudioEndpointInfo as isize,
    AudioEndpointPlayTone = AbiSyscallNumber::AudioEndpointPlayTone as isize,
    AudioEndpointStop = AbiSyscallNumber::AudioEndpointStop as isize,
    MemoryMap = AbiSyscallNumber::MemoryMap as isize,
    TaskSpawnImage = AbiSyscallNumber::TaskSpawnImage as isize,
    BlockDeviceInfo = AbiSyscallNumber::BlockDeviceInfo as isize,
    BlockDeviceRead = AbiSyscallNumber::BlockDeviceRead as isize,
    BlockDeviceWrite = AbiSyscallNumber::BlockDeviceWrite as isize,
    MemoryInfo = AbiSyscallNumber::MemoryInfo as isize,
    MemoryMapRange = AbiSyscallNumber::MemoryMapRange as isize,
    EventCreate = AbiSyscallNumber::EventCreate as isize,
    EventSignal = AbiSyscallNumber::EventSignal as isize,
    EventReset = AbiSyscallNumber::EventReset as isize,
    ObjectInfo = AbiSyscallNumber::ObjectInfo as isize,
    ObjectWait = AbiSyscallNumber::ObjectWait as isize,
    KernelEventQueryInfo = AbiSyscallNumber::KernelEventQueryInfo as isize,
    KernelEventQueryRecord = AbiSyscallNumber::KernelEventQueryRecord as isize,
    DisplayOutputPresentDamage = AbiSyscallNumber::DisplayOutputPresentDamage as isize,
    MemoryUnmap = AbiSyscallNumber::MemoryUnmap as isize,
    MemoryProtect = AbiSyscallNumber::MemoryProtect as isize,
    MemoryQuery = AbiSyscallNumber::MemoryQuery as isize,
    FaultHandlerRegister = AbiSyscallNumber::FaultHandlerRegister as isize,
    FaultHandlerUnregister = AbiSyscallNumber::FaultHandlerUnregister as isize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyscallSnapshot {
    pub dispatched: u64,
    pub rejected: u64,
}

pub trait SyscallDispatcher {
    fn dispatch(&self, number: SyscallNumber, context: &SyscallContext) -> SyscallReturn;
}
