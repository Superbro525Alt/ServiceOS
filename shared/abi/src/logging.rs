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
    Security = 20,
    Kernel = 21,
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
    PackageRepositoryAdded = 61,
    PackageRepositorySynced = 62,
    PackageRepositorySyncFailed = 63,
    PackageRepairCompleted = 64,
    PackageGarbageCollected = 65,
    SecurityPolicyChanged = 66,
    SecurityLaunchDenied = 67,
    RuntimeApprovalPending = 68,
    RuntimeApprovalChanged = 69,
    KernelTrap = 70,
    KernelPressureChanged = 71,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogTag {
    Record = 0x100,
    QueryInfoRequest = 0x101,
    QueryInfoReply = 0x102,
    QueryRecordRequest = 0x103,
    QueryRecordReply = 0x104,
    SubscribeRequest = 0x105,
    SubscribeReply = 0x106,
    StreamRecord = 0x107,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogQueryStatus {
    Ok = 0,
    NotFound = 1,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogStatus {
    Ok = 0,
    Busy = 1,
}

pub const LOG_FILTER_ANY: u64 = u64::MAX;
