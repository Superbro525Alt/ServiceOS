#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecurityTag {
    PolicyListRequest = 0xe00,
    PolicyListReply = 0xe01,
    PolicyInfoRequest = 0xe02,
    PolicyInfoReply = 0xe03,
    PolicySetRequest = 0xe04,
    PolicySetReply = 0xe05,
    AuditListRequest = 0xe06,
    AuditListReply = 0xe07,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecurityStatus {
    Ok = 0,
    NotFound = 1,
    Busy = 2,
    Denied = 3,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionPolicyState {
    DefaultAllow = 1,
    Allowed = 2,
    Blocked = 3,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecurityAuditKind {
    PolicyChanged = 1,
    LaunchDenied = 2,
    RuntimeApprovalRequested = 3,
    RuntimeApprovalChanged = 4,
}

pub mod app_permission {
    pub const CONFIG: u32 = 1 << 0;
    pub const STORAGE: u32 = 1 << 1;
    pub const STATUS: u32 = 1 << 2;
    pub const PACKAGE: u32 = 1 << 3;
    pub const NETWORK: u32 = 1 << 4;
    pub const AUDIO: u32 = 1 << 5;
    pub const TERMINAL: u32 = 1 << 6;
    pub const CLIPBOARD: u32 = 1 << 7;
}
