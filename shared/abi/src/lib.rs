#![no_std]

pub type Handle = u32;

pub const INVALID_HANDLE: Handle = 0;
pub const IPC_MAX_WORDS: usize = 16;
pub const IPC_MAX_HANDLES: usize = 8;

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

pub const IPC_FLAG_NONBLOCK: u32 = 1 << 0;

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootstrapPlatform {
    Unknown = 0,
    QemuVirtio = 1,
    Raspi5 = 2,
}

pub mod bootstrap_resource {
    pub const NETWORK: u64 = 1 << 0;
    pub const DISPLAY: u64 = 1 << 1;
    pub const INPUT: u64 = 1 << 2;
    pub const AUDIO: u64 = 1 << 3;
    pub const BLOCK: u64 = 1 << 4;
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockDeviceBackend {
    Unknown = 0,
    VirtioPci = 1,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockDeviceInfo {
    pub backend: u32,
    pub writable: u32,
    pub block_size: u32,
    pub reserved: u32,
    pub block_count: u64,
    pub read_ops: u64,
    pub write_ops: u64,
}

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
    DesktopShellService = 14,
    SettingsApp = 15,
    FilesApp = 16,
    MonitorApp = 17,
    TerminalService = 18,
    TerminalApp = 19,
    AudioService = 20,
    RuntimeService = 21,
    PosixHostTool = 22,
    DeveloperService = 23,
    CrossBuilderTool = 24,
    ClipboardService = 25,
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
    DesktopShell = 13,
    Terminal = 14,
    Audio = 15,
    Runtime = 16,
    Developer = 17,
    Clipboard = 18,
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
    Desktop = 15,
    App = 16,
    Audio = 17,
    Runtime = 18,
    Developer = 19,
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
    DesktopReady = 34,
    DesktopAppLaunched = 35,
    DesktopAppExited = 36,
    DesktopFocusChanged = 37,
    AppRendered = 38,
    InputSourceReady = 39,
    InputKeyDelivered = 40,
    NetworkLeaseChanged = 41,
    NetworkSocketOpened = 42,
    NetworkSocketClosed = 43,
    TerminalSessionOpened = 44,
    TerminalSessionClosed = 45,
    AudioEndpointReady = 46,
    AudioStreamOpened = 47,
    AudioStreamStarted = 48,
    AudioStreamStopped = 49,
    AudioStreamClosed = 50,
    RuntimeEnvironmentCreated = 51,
    RuntimeEnvironmentDestroyed = 52,
    RuntimeLaunchStarted = 53,
    RuntimeLaunchExited = 54,
    RuntimeMappedRead = 55,
    DeveloperCatalogLoaded = 56,
    DeveloperBuildStarted = 57,
    DeveloperBuildFinished = 58,
    DeveloperBuildFailed = 59,
    DeveloperArtifactOpened = 60,
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
    WriteRequest = 0x302,
    WriteReply = 0x303,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigStatus {
    Ok = 0,
    NotFound = 1,
    Denied = 2,
    Invalid = 3,
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
    DirectoryListRequest = 0x507,
    DirectoryListReply = 0x508,
    DirectoryOpenRequest = 0x509,
    DirectoryOpenReply = 0x50a,
    DirectoryCreateRequest = 0x50b,
    DirectoryCreateReply = 0x50c,
    DirectoryRemoveRequest = 0x50d,
    DirectoryRemoveReply = 0x50e,
    DirectoryOpenFileRequest = 0x50f,
    DirectoryOpenFileReply = 0x510,
    WriteRequest = 0x511,
    WriteReply = 0x512,
    DirectoryReadRequest = 0x513,
    DirectoryReadReply = 0x514,
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
    Denied = 6,
    AlreadyExists = 7,
    NotDirectory = 8,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageEntryKind {
    File = 0,
    Directory = 1,
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
    LaunchImageRequest = 0x60c,
    LaunchImageReply = 0x60d,
    LaunchStoredImageRequest = 0x60e,
    LaunchStoredImageReply = 0x60f,
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
    NetworkDynamicIpv4 = 9,
    NetworkDnsServer = 10,
    NetworkDnsQueryTimeoutTicks = 11,
    NetworkDhcpAcquireTimeoutTicks = 12,
    NetworkTcpConnectTimeoutTicks = 13,
    NetworkTcpIdleTimeoutTicks = 14,
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
pub enum RuntimeTag {
    EnvListRequest = 0xc00,
    EnvListReply = 0xc01,
    EnvStatusRequest = 0xc02,
    EnvStatusReply = 0xc03,
    EnvCreateRequest = 0xc04,
    EnvCreateReply = 0xc05,
    EnvDestroyRequest = 0xc06,
    EnvDestroyReply = 0xc07,
    EnvMountListRequest = 0xc08,
    EnvMountListReply = 0xc09,
    EnvVarListRequest = 0xc0a,
    EnvVarListReply = 0xc0b,
    RunLaunchRequest = 0xc0c,
    RunLaunchReply = 0xc0d,
    RunListRequest = 0xc0e,
    RunListReply = 0xc0f,
    RunStatusRequest = 0xc10,
    RunStatusReply = 0xc11,
    SessionInfoRequest = 0xc12,
    SessionInfoReply = 0xc13,
    SessionMountListRequest = 0xc14,
    SessionMountListReply = 0xc15,
    SessionVarListRequest = 0xc16,
    SessionVarListReply = 0xc17,
    SessionReadFileRequest = 0xc18,
    SessionReadFileReply = 0xc19,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeveloperTag {
    ToolchainListRequest = 0xd00,
    ToolchainListReply = 0xd01,
    ToolchainInfoRequest = 0xd02,
    ToolchainInfoReply = 0xd03,
    WorkspaceListRequest = 0xd04,
    WorkspaceListReply = 0xd05,
    WorkspaceInfoRequest = 0xd06,
    WorkspaceInfoReply = 0xd07,
    BuildRequest = 0xd08,
    BuildReply = 0xd09,
    JobListRequest = 0xd0a,
    JobListReply = 0xd0b,
    JobInfoRequest = 0xd0c,
    JobInfoReply = 0xd0d,
    ArtifactOpenRequest = 0xd0e,
    ArtifactOpenReply = 0xd0f,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeveloperStatus {
    Ok = 0,
    NotFound = 1,
    Busy = 2,
    Denied = 3,
    Unsupported = 4,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeveloperTarget {
    NativeX64 = 1,
    LinuxX64 = 2,
    WindowsX64 = 3,
    MacosX64 = 4,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeveloperToolchainState {
    Installed = 1,
    RemoteOnly = 2,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeveloperArtifactFormat {
    ServiceOsFlat = 1,
    Elf64 = 2,
    Pe32Plus = 3,
    MachO64 = 4,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeveloperJobState {
    Queued = 1,
    Running = 2,
    Succeeded = 3,
    Failed = 4,
    Unsupported = 5,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeStatus {
    Ok = 0,
    NotFound = 1,
    Busy = 2,
    Denied = 3,
    InvalidPath = 4,
    Unsupported = 5,
    Closed = 6,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeKind {
    Posix = 1,
    Windows = 2,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeEnvState {
    Ready = 1,
    Busy = 2,
    Destroyed = 3,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeRunState {
    Launching = 1,
    Running = 2,
    Exited = 3,
    Failed = 4,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeWorkloadKind {
    Inspect = 1,
    Env = 2,
    Mounts = 3,
    Cat = 4,
}

pub mod runtime_capability {
    pub const FILE_READ: u32 = 1 << 0;
    pub const TERMINAL_IO: u32 = 1 << 1;
    pub const NETWORK: u32 = 1 << 2;
    pub const GRAPHICS: u32 = 1 << 3;
    pub const AUDIO: u32 = 1 << 4;
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

pub const INPUT_SOURCE_FLAG_NONBLOCK: u32 = 1 << 0;

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioEndpointBackend {
    Unknown = 0,
    PcSpeaker = 1,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioEndpointDirection {
    Output = 1,
    Input = 2,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioEndpointState {
    Offline = 0,
    Idle = 1,
    Active = 2,
}

pub mod audio_capability {
    pub const PLAYBACK: u32 = 1 << 0;
    pub const CAPTURE: u32 = 1 << 1;
    pub const TONE: u32 = 1 << 2;
    pub const PCM: u32 = 1 << 3;
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioEndpointInfo {
    pub backend: u32,
    pub direction: u32,
    pub state: u32,
    pub capabilities: u32,
    pub nominal_rate_hz: u32,
    pub channels: u32,
    pub min_frequency_hz: u32,
    pub max_frequency_hz: u32,
    pub current_frequency_hz: u32,
    pub reserved: u32,
    pub play_count: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioToneRequest {
    pub frequency_hz: u32,
    pub duration_ticks: u32,
    pub volume: u16,
    pub flags: u16,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputSourceBackend {
    Unknown = 0,
    VirtioPci = 1,
}

pub mod input_capability {
    pub const POINTER: u32 = 1 << 0;
    pub const KEYBOARD: u32 = 1 << 1;
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputSourceInfo {
    pub backend: u32,
    pub capabilities: u32,
    pub device_count: u32,
    pub pending_events: u32,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputEventKind {
    PointerMotion = 1,
    PointerButton = 2,
    Key = 3,
    PointerDelta = 4,
    PointerScroll = 5,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputButton {
    Left = 1,
    Right = 2,
    Middle = 3,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputEventInfo {
    pub kind: u32,
    pub code: u32,
    pub value0: i32,
    pub value1: i32,
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
    SocketOpenRequest = 0x808,
    SocketOpenReply = 0x809,
    SocketListRequest = 0x80a,
    SocketListReply = 0x80b,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioTag {
    EndpointListRequest = 0x880,
    EndpointListReply = 0x881,
    EndpointStatusRequest = 0x882,
    EndpointStatusReply = 0x883,
    StreamOpenRequest = 0x884,
    StreamOpenReply = 0x885,
    StreamListRequest = 0x886,
    StreamListReply = 0x887,
    StreamStatusRequest = 0x888,
    StreamStatusReply = 0x889,
    StreamPlayToneRequest = 0x88a,
    StreamPlayToneReply = 0x88b,
    StreamCloseRequest = 0x88c,
    StreamCloseReply = 0x88d,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioStatus {
    Ok = 0,
    NotFound = 1,
    Busy = 2,
    Unsupported = 3,
    Denied = 4,
    CapacityExceeded = 5,
    Closed = 6,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioStreamDirection {
    Playback = 1,
    Capture = 2,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioStreamState {
    Idle = 1,
    Active = 2,
    Closed = 3,
    Failed = 4,
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
    Denied = 7,
    CapacityExceeded = 8,
    Closed = 9,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkConfigMode {
    Static = 1,
    Dynamic = 2,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkConfigState {
    Pending = 1,
    Configured = 2,
    FallbackStatic = 3,
    Failed = 4,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkSocketKind {
    TcpStream = 1,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkSocketState {
    Connecting = 1,
    Established = 2,
    Closing = 3,
    Closed = 4,
    Failed = 5,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkSocketTag {
    StatusRequest = 0x820,
    StatusReply = 0x821,
    SendRequest = 0x822,
    SendReply = 0x823,
    ReceiveRequest = 0x824,
    ReceiveReply = 0x825,
    CloseRequest = 0x826,
    CloseReply = 0x827,
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
pub enum DesktopTag {
    StatusRequest = 0xa00,
    StatusReply = 0xa01,
    ListAppsRequest = 0xa02,
    ListAppsReply = 0xa03,
    LaunchAppRequest = 0xa04,
    LaunchAppReply = 0xa05,
    FocusAppRequest = 0xa06,
    FocusAppReply = 0xa07,
    ListWindowsRequest = 0xa08,
    ListWindowsReply = 0xa09,
    WindowActionRequest = 0xa0a,
    WindowActionReply = 0xa0b,
    InputRequest = 0xa0c,
    InputReply = 0xa0d,
    NotifyRequest = 0xa0e,
    NotifyReply = 0xa0f,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DesktopStatus {
    Ok = 0,
    NotFound = 1,
    Busy = 2,
    Denied = 3,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum DesktopAppId {
    Settings = 1,
    Files = 2,
    Monitor = 3,
    Terminal = 4,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalTag {
    SessionOpenRequest = 0xb00,
    SessionOpenReply = 0xb01,
    SessionListRequest = 0xb02,
    SessionListReply = 0xb03,
    SessionStatusRequest = 0xb04,
    SessionStatusReply = 0xb05,
    SessionInput = 0xb06,
    SessionOutput = 0xb07,
    SessionResize = 0xb08,
    SessionClose = 0xb09,
    SessionClosed = 0xb0a,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalStatus {
    Ok = 0,
    Busy = 1,
    NotFound = 2,
    Denied = 3,
    Closed = 4,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClipboardTag {
    ReadRequest = 0xb20,
    ReadReply = 0xb21,
    WriteRequest = 0xb22,
    WriteReply = 0xb23,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClipboardStatus {
    Ok = 0,
    NotFound = 1,
    Denied = 2,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DesktopWindowAction {
    Focus = 1,
    Close = 2,
    Minimize = 3,
    Restore = 4,
    Move = 5,
    Resize = 6,
    FocusNext = 7,
    Maximize = 8,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DesktopInputAction {
    PointerDown = 1,
    PointerMove = 2,
    PointerUp = 3,
    Click = 4,
    KeyDown = 5,
    KeyUp = 6,
    TextInput = 7,
    PointerScroll = 8,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DesktopDragMode {
    None = 0,
    Move = 1,
    Resize = 2,
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
    ClearSceneRequest = 0x927,
    ClearSceneReply = 0x928,
    SetRectRequest = 0x929,
    SetRectReply = 0x92a,
    SetLabelRequest = 0x92b,
    SetLabelReply = 0x92c,
    AttachBufferRequest = 0x92d,
    AttachBufferReply = 0x92e,
    PresentBufferRequest = 0x92f,
    PresentBufferReply = 0x930,
    ReleaseBufferRequest = 0x931,
    ReleaseBufferReply = 0x932,
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
    Hardware = 2,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppControlTag {
    FocusChanged = 0xac0,
    Resize = 0xac1,
    Close = 0xac2,
    Pointer = 0xac3,
    Key = 0xac4,
    Text = 0xac5,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppPointerAction {
    Down = 1,
    Move = 2,
    Up = 3,
    Scroll = 4,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppKeyAction {
    Down = 1,
    Up = 2,
}
