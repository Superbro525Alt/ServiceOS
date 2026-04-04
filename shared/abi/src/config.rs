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
