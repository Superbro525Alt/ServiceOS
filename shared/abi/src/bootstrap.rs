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
    SoftwareCenterApp = 26,
    SecurityService = 27,
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
    Security = 19,
}
