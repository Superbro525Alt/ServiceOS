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
