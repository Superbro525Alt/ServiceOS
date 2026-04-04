pub type Handle = u32;

pub const INVALID_HANDLE: Handle = 0;
pub const IPC_MAX_WORDS: usize = 16;
pub const IPC_MAX_HANDLES: usize = 8;
pub const IPC_FLAG_NONBLOCK: u32 = 1 << 0;

pub mod rights {
    pub const NONE: u64 = 0;
    pub const READ: u64 = 1 << 0;
    pub const WRITE: u64 = 1 << 1;
    pub const MAP: u64 = 1 << 2;
    pub const SIGNAL: u64 = 1 << 3;
    pub const WAIT: u64 = 1 << 4;
    pub const SEND: u64 = 1 << 5;
    pub const RECEIVE: u64 = 1 << 6;
    pub const DUPLICATE: u64 = 1 << 7;
    pub const TRANSFER: u64 = 1 << 8;
    pub const MANAGE: u64 = 1 << 9;
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyscallNumber {
    AbiVersion = 0,
    MonotonicNow = 1,
    ThreadExit = 2,
    YieldCurrent = 3,
    DebugLogWrite = 4,
    ChannelCreate = 5,
    ChannelSend = 6,
    ChannelReceive = 7,
    HandleDuplicate = 8,
    HandleClose = 9,
    ServiceSpawn = 10,
    TaskStatus = 11,
    MemoryRead = 12,
    DebugConsoleRead = 13,
    DebugConsoleWrite = 14,
    PacketInterfaceInfo = 15,
    PacketInterfaceReceive = 16,
    PacketInterfaceTransmit = 17,
    DisplayOutputInfo = 18,
    DisplayOutputPresent = 19,
    InputSourceInfo = 20,
    InputSourceReceive = 21,
    MemoryCreate = 22,
    MemoryWrite = 23,
    AudioEndpointInfo = 24,
    AudioEndpointPlayTone = 25,
    AudioEndpointStop = 26,
    MemoryMap = 27,
    TaskSpawnImage = 28,
    BlockDeviceInfo = 29,
    BlockDeviceRead = 30,
    BlockDeviceWrite = 31,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyscallErrorCode {
    Ok = 0,
    Unsupported = 1,
    InvalidCall = 2,
    PermissionDenied = 3,
    NotInitialized = 4,
    InvalidArgument = 5,
    BufferTooSmall = 6,
    QueueEmpty = 7,
    NotFound = 8,
    Busy = 9,
    CapacityExceeded = 10,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HandlePair {
    pub first: Handle,
    pub second: Handle,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RawMessage {
    pub tag: u32,
    pub word_count: u32,
    pub handle_count: u32,
    pub flags: u32,
    pub words: [u64; IPC_MAX_WORDS],
    pub handles: [Handle; IPC_MAX_HANDLES],
    pub handle_rights: [u64; IPC_MAX_HANDLES],
}

impl RawMessage {
    pub const fn empty(tag: u32) -> Self {
        Self {
            tag,
            word_count: 0,
            handle_count: 0,
            flags: 0,
            words: [0; IPC_MAX_WORDS],
            handles: [INVALID_HANDLE; IPC_MAX_HANDLES],
            handle_rights: [0; IPC_MAX_HANDLES],
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlTag {
    Startup = 1,
    Register = 2,
    LookupRequest = 3,
    LookupReply = 4,
    Lifecycle = 5,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LookupStatus {
    Ok = 0,
    Denied = 1,
    Unavailable = 2,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleEvent {
    Starting = 1,
    Ready = 2,
    Failed = 3,
    Restarting = 4,
    Stopped = 5,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskStateCode {
    Running = 1,
    Exited = 2,
    Faulted = 3,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskStatus {
    pub state: TaskStateCode,
    pub exit_code: u64,
}
