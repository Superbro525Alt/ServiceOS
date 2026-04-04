#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatusTag {
    SnapshotRequest = 0x400,
    SnapshotReply = 0x401,
    ServiceReport = 0x402,
    ServiceQueryRequest = 0x403,
    ServiceQueryReply = 0x404,
    ServiceListRequest = 0x405,
    ServiceListReply = 0x406,
    SubscribeRequest = 0x407,
    SubscribeReply = 0x408,
    StreamEvent = 0x409,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatusResult {
    Ok = 0,
    NotFound = 1,
    Busy = 2,
    Denied = 3,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatusHealth {
    Unknown = 0,
    Healthy = 1,
    Degraded = 2,
    Failing = 3,
    Recovering = 4,
    Dormant = 5,
}

pub mod status_detail_kind {
    pub const NONE: u32 = 0;
    pub const LIFECYCLE: u32 = 1;
    pub const BLOCKED_DEPENDENCY: u32 = 2;
    pub const RESTART_BACKOFF: u32 = 3;
    pub const HEARTBEAT: u32 = 4;
}
