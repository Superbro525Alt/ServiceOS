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
    EnvDecisionRequest = 0xc1a,
    EnvDecisionReply = 0xc1b,
    AuditListRequest = 0xc1c,
    AuditListReply = 0xc1d,
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
    PendingApproval = 7,
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
    PendingApproval = 4,
    Denied = 5,
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
