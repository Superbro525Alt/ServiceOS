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
    LaunchPolicyListRequest = 0x610,
    LaunchPolicyListReply = 0x611,
    LaunchPolicySetRequest = 0x612,
    LaunchPolicySetReply = 0x613,
    LaunchAuditListRequest = 0x614,
    LaunchAuditListReply = 0x615,
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
