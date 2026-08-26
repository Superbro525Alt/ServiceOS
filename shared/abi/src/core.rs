pub type Handle = u32;

pub const INVALID_HANDLE: Handle = 0;
pub const IPC_MAX_WORDS: usize = 16;
pub const IPC_MAX_HANDLES: usize = 8;
pub const IPC_FLAG_NONBLOCK: u32 = 1 << 0;
pub const IPC_FLAG_RECEIVE_TIMEOUT: u32 = 1 << 1;
pub const OBJECT_WAIT_FLAG_NONBLOCK: u32 = 1 << 0;
pub const PIPE_FLAG_NONBLOCK: u32 = 1 << 0;

pub mod memory_map_flags {
    pub const WRITABLE: u32 = 1 << 0;
    pub const FIXED: u32 = 1 << 1;
}

pub mod object_state_flags {
    pub const READY: u32 = 1 << 0;
    pub const SIGNALED: u32 = 1 << 1;
    pub const ARMED: u32 = 1 << 2;
    pub const WRITABLE: u32 = 1 << 3;
    pub const RUNNING: u32 = 1 << 4;
    pub const EXITED: u32 = 1 << 5;
    pub const FAULTED: u32 = 1 << 6;
}

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
    MemoryInfo = 32,
    MemoryMapRange = 33,
    EventCreate = 34,
    EventSignal = 35,
    EventReset = 36,
    ObjectInfo = 37,
    ObjectWait = 38,
    KernelEventQueryInfo = 39,
    KernelEventQueryRecord = 40,
    DisplayOutputPresentDamage = 41,
    MemoryUnmap = 42,
    MemoryProtect = 43,
    MemoryQuery = 44,
    FaultHandlerRegister = 45,
    FaultHandlerUnregister = 46,
    TaskLoadedLibraries = 47,
    AudioEndpointPcmWrite = 48,
    PipeCreate = 49,
    PipeRead = 50,
    PipeWrite = 51,
    PacketInterfaceRingSetup = 52,
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
    BrokenPipe = 11,
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

/// One companion library image mapped into a task's address space by the
/// loader (extended flat-image headers only). Returned by
/// [`SyscallNumber::TaskLoadedLibraries`].
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskLoadedLibrary {
    pub image_id: u32,
    pub _pad: u32,
    pub base: u64,
    pub mapped_bytes: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryObjectInfo {
    pub size_bytes: usize,
    pub page_count: usize,
    pub writable: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryMapRequest {
    pub offset_bytes: usize,
    pub length_bytes: usize,
    pub address_hint: u64,
    pub flags: u32,
    pub reserved: u32,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectKindCode {
    Task = 1,
    Thread = 2,
    ChannelEndpoint = 3,
    Event = 4,
    Timer = 5,
    MemoryObject = 6,
    BootstrapCapability = 7,
    PacketInterface = 8,
    DisplayOutput = 9,
    InputSource = 10,
    AudioEndpoint = 11,
    BlockDevice = 12,
    Pipe = 13,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObjectInfo {
    pub object_id: u64,
    pub kind: ObjectKindCode,
    pub state_flags: u32,
    pub reserved: u32,
    pub detail0: u64,
    pub detail1: u64,
    pub detail2: u64,
    pub detail3: u64,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KernelEventKind {
    Trap = 1,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KernelEventRecord {
    pub sequence: u64,
    pub kind: KernelEventKind,
    pub reserved: u32,
    pub tick: u64,
    pub detail0: u64,
    pub detail1: u64,
    pub detail2: u64,
    pub detail3: u64,
    pub detail4: u64,
}
