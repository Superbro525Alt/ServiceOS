#![no_std]

pub type Handle = u32;

pub const INVALID_HANDLE: Handle = 0;
pub const IPC_MAX_WORDS: usize = 16;
pub const IPC_MAX_HANDLES: usize = 4;

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

pub const IPC_FLAG_NONBLOCK: u32 = 1 << 0;

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceImageId {
    RootManager = 1,
    StorageService = 2,
    ConsoleService = 3,
    ConfigService = 4,
    LogService = 5,
    StatusService = 6,
    ShellService = 7,
    SysinfoTool = 8,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum ServiceId {
    RootManager = 1,
    Storage = 2,
    Console = 3,
    Config = 4,
    Log = 5,
    Status = 6,
    Shell = 7,
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
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskStatus {
    pub state: TaskStateCode,
    pub exit_code: u64,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum LogSeverity {
    Trace = 1,
    Debug = 2,
    Info = 3,
    Warn = 4,
    Error = 5,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogDomain {
    Bootstrap = 1,
    ServiceManager = 2,
    Service = 3,
    Storage = 4,
    Log = 5,
    Config = 6,
    Console = 7,
    Status = 8,
    Ipc = 9,
    Shell = 10,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogEvent {
    ServiceStarted = 1,
    ServiceReady = 2,
    ServiceFailed = 3,
    ServiceRestarting = 4,
    ConfigLoaded = 5,
    ConfigRead = 6,
    ConsoleWrite = 7,
    StatusStarted = 8,
    StatusHeartbeat = 9,
    LookupGranted = 10,
    StorageMounted = 11,
    ManifestLoaded = 12,
    ResourceOpened = 13,
    SessionOpened = 14,
    ShellCommand = 15,
    ToolLaunched = 16,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogTag {
    Record = 0x100,
    QueryInfoRequest = 0x101,
    QueryInfoReply = 0x102,
    QueryRecordRequest = 0x103,
    QueryRecordReply = 0x104,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConsoleTag {
    WriteRecord = 0x200,
    SessionOpenRequest = 0x201,
    SessionOpenReply = 0x202,
    SessionWriteText = 0x203,
    SessionReadLineRequest = 0x204,
    SessionReadLineReply = 0x205,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigTag {
    ReadRequest = 0x300,
    ReadReply = 0x301,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatusTag {
    SnapshotRequest = 0x400,
    SnapshotReply = 0x401,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageTag {
    OpenRequest = 0x500,
    OpenReply = 0x501,
    ReadRequest = 0x502,
    ReadReply = 0x503,
    ListRequest = 0x504,
    ListReply = 0x505,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageStatus {
    Ok = 0,
    NotFound = 1,
    InvalidPath = 2,
    InvalidOffset = 3,
    End = 4,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogQueryStatus {
    Ok = 0,
    NotFound = 1,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagerTag {
    ListServicesRequest = 0x600,
    ListServicesReply = 0x601,
    ServiceStatusRequest = 0x602,
    ServiceStatusReply = 0x603,
    ServiceActionRequest = 0x604,
    ServiceActionReply = 0x605,
    LaunchRequest = 0x606,
    LaunchReply = 0x607,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagerStatus {
    Ok = 0,
    Denied = 1,
    NotFound = 2,
    Busy = 3,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagerAction {
    Restart = 1,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagerServicePhase {
    Dormant = 0,
    Starting = 1,
    Ready = 2,
    Exited = 3,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigKey {
    LogMinimumSeverity = 1,
    StatusHeartbeatTicks = 2,
    StatusConsoleMirror = 3,
    StatusHeartbeatLogPeriod = 4,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigValueKind {
    Unsigned = 1,
}
