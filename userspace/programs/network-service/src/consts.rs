pub(crate) const MAX_HOSTS: usize = 8;
pub(crate) const MAX_HOSTNAME_BYTES: usize = 48;
pub(crate) const MAX_HOSTS_RESOURCE_BYTES: usize = 256;
pub(crate) const MAX_FRAME_BYTES: usize = 1536;
pub(crate) const MAX_DNS_QUERY_SLOTS: usize = 4;
pub(crate) const MAX_TCP_SOCKETS: usize = 2;
pub(crate) const TCP_SOCKET_BUFFER_BYTES: usize = 1024;
pub(crate) const PING_IDENTIFIER: u16 = 0x534f;
pub(crate) const EPHEMERAL_PORT_BASE: u16 = 49_152;
pub(crate) const MAX_SOCKET_INLINE_BYTES: usize = (serviceos_userspace_runtime::IPC_MAX_WORDS - 2) * 8;

pub(crate) const MAX_UDP_SOCKETS: usize = 4;
pub(crate) const UDP_DATAGRAM_BUFFER_BYTES: usize = 2048;
pub(crate) const MAX_TCP_LISTENERS: usize = 2;
pub(crate) const TCP_ACCEPT_BACKLOG: usize = 2;

pub(crate) const LOOPBACK_ADDRESS: smoltcp::wire::Ipv4Address =
    smoltcp::wire::Ipv4Address::new(127, 0, 0, 1);

pub(crate) const SELFTEST_UDP_PORT_A: u16 = 40_123;
pub(crate) const SELFTEST_UDP_PORT_B: u16 = 40_124;
pub(crate) const SELFTEST_TCP_PORT: u16 = 40_125;
pub(crate) const SELFTEST_POLL_LIMIT: usize = 4096;
