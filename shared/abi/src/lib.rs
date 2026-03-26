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
    PacketInterfaceInfo = 15,
    PacketInterfaceReceive = 16,
    PacketInterfaceTransmit = 17,
    DisplayOutputInfo = 18,
    DisplayOutputPresent = 19,
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
    PackageService = 9,
    AnnounceService = 10,
    NetworkService = 11,
    GraphicsService = 12,
    SessionService = 13,
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
    Package = 8,
    Announce = 9,
    Network = 10,
    Graphics = 11,
    Session = 12,
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
    Package = 11,
    Network = 12,
    Graphics = 13,
    Session = 14,
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
    PackageCatalogLoaded = 17,
    PackageInstalled = 18,
    PackageUpdated = 19,
    PackageRemoved = 20,
    PackageRolledBack = 21,
    PackageActivationFailed = 22,
    NetworkInterfaceReady = 23,
    NetworkAddressConfigured = 24,
    NetworkResolveCompleted = 25,
    NetworkProbeCompleted = 26,
    NetworkLinkChanged = 27,
    DisplayOutputReady = 28,
    SurfaceCreated = 29,
    SurfaceUpdated = 30,
    CompositorPresented = 31,
    SessionReady = 32,
    SessionFocusChanged = 33,
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
    CloseRequest = 0x506,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageStatus {
    Ok = 0,
    NotFound = 1,
    InvalidPath = 2,
    InvalidOffset = 3,
    End = 4,
    Busy = 5,
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
    ActivateRequest = 0x608,
    ActivateReply = 0x609,
    DeactivateRequest = 0x60a,
    DeactivateReply = 0x60b,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagerStatus {
    Ok = 0,
    Denied = 1,
    NotFound = 2,
    Busy = 3,
    Failed = 4,
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
    NetworkIpv4Address = 5,
    NetworkIpv4PrefixLength = 6,
    NetworkIpv4Gateway = 7,
    NetworkProbeTimeoutTicks = 8,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigValueKind {
    Unsigned = 1,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackageTag {
    ListRequest = 0x700,
    ListReply = 0x701,
    InfoRequest = 0x702,
    InfoReply = 0x703,
    InstallRequest = 0x704,
    InstallReply = 0x705,
    RemoveRequest = 0x706,
    RemoveReply = 0x707,
    UpdateRequest = 0x708,
    UpdateReply = 0x709,
    RollbackRequest = 0x70a,
    RollbackReply = 0x70b,
    HistoryRequest = 0x70c,
    HistoryReply = 0x70d,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackageStatus {
    Ok = 0,
    NotFound = 1,
    AlreadyInstalled = 2,
    NotInstalled = 3,
    Busy = 4,
    Denied = 5,
    IntegrityFailed = 6,
    End = 7,
    NoChange = 8,
    NoRollback = 9,
}

pub const PACKET_INTERFACE_FLAG_NONBLOCK: u32 = 1 << 0;

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PacketInterfaceBackend {
    Unknown = 0,
    VirtioPci = 1,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PacketInterfaceLinkState {
    Down = 0,
    Up = 1,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PacketInterfaceInfo {
    pub backend: u32,
    pub link_state: u32,
    pub mtu: u32,
    pub rx_ready: u32,
    pub mac: [u8; 6],
    pub reserved: [u8; 2],
    pub rx_packets: u64,
    pub tx_packets: u64,
    pub dropped_packets: u64,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayOutputBackend {
    Unknown = 0,
    BootFramebuffer = 1,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayOutputState {
    Disconnected = 0,
    Connected = 1,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayPixelFormat {
    Unknown = 0,
    Xrgb8888 = 1,
    Bgrx8888 = 2,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DisplayOutputInfo {
    pub backend: u32,
    pub state: u32,
    pub pixel_format: u32,
    pub reserved: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub bytes_per_pixel: u32,
    pub byte_len: u64,
    pub present_count: u64,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkTag {
    InterfaceListRequest = 0x800,
    InterfaceListReply = 0x801,
    InterfaceStatusRequest = 0x802,
    InterfaceStatusReply = 0x803,
    ResolveRequest = 0x804,
    ResolveReply = 0x805,
    PingRequest = 0x806,
    PingReply = 0x807,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkStatus {
    Ok = 0,
    NotFound = 1,
    Busy = 2,
    InvalidTarget = 3,
    Timeout = 4,
    End = 5,
    Unsupported = 6,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GraphicsTag {
    OutputListRequest = 0x900,
    OutputListReply = 0x901,
    OutputStatusRequest = 0x902,
    OutputStatusReply = 0x903,
    SurfaceCreateRequest = 0x904,
    SurfaceCreateReply = 0x905,
    SurfaceListRequest = 0x906,
    SurfaceListReply = 0x907,
    SurfaceStatusRequest = 0x908,
    SurfaceStatusReply = 0x909,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GraphicsStatus {
    Ok = 0,
    NotFound = 1,
    Busy = 2,
    Denied = 3,
    CapacityExceeded = 4,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SurfaceTag {
    SetGeometryRequest = 0x920,
    SetGeometryReply = 0x921,
    SetFillRequest = 0x922,
    SetFillReply = 0x923,
    SetVisibilityRequest = 0x924,
    SetVisibilityReply = 0x925,
    CloseRequest = 0x926,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionTag {
    ListRequest = 0x980,
    ListReply = 0x981,
    StatusRequest = 0x982,
    StatusReply = 0x983,
    FocusRequest = 0x984,
    FocusReply = 0x985,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionStatus {
    Ok = 0,
    NotFound = 1,
    Busy = 2,
    Denied = 3,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionInputSource {
    None = 0,
    ServiceControl = 1,
}
