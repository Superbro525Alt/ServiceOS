use smoltcp::{iface::SocketHandle, wire::Ipv4Address};

use serviceos_userspace_runtime as rt;
use rt::{NetworkConfigMode, NetworkConfigState, NetworkSocketState};

use crate::consts::MAX_HOSTNAME_BYTES;

#[derive(Clone, Copy)]
pub(crate) struct HostEntry {
    pub(crate) name: [u8; MAX_HOSTNAME_BYTES],
    pub(crate) name_len: usize,
    pub(crate) address: Ipv4Address,
}

impl HostEntry {
    pub(crate) const fn empty() -> Self {
        Self {
            name: [0; MAX_HOSTNAME_BYTES],
            name_len: 0,
            address: Ipv4Address::UNSPECIFIED,
        }
    }

    pub(crate) fn matches(&self, target: &str) -> bool {
        self.name_len == target.len() && self.name[..self.name_len] == *target.as_bytes()
    }
}

#[derive(Clone, Copy)]
pub(crate) struct NetworkConfig {
    pub(crate) static_address: Ipv4Address,
    pub(crate) static_prefix_len: u8,
    pub(crate) static_gateway: Ipv4Address,
    pub(crate) dynamic_ipv4: bool,
    pub(crate) dns_server: Ipv4Address,
    pub(crate) probe_timeout_ticks: u64,
    pub(crate) dns_query_timeout_ticks: u64,
    pub(crate) dhcp_acquire_timeout_ticks: u64,
    pub(crate) tcp_connect_timeout_ticks: u64,
    pub(crate) tcp_idle_timeout_ticks: u64,
}

#[derive(Clone, Copy)]
pub(crate) struct InterfaceRuntimeState {
    pub(crate) mode: NetworkConfigMode,
    pub(crate) state: NetworkConfigState,
    pub(crate) address: Ipv4Address,
    pub(crate) prefix_len: u8,
    pub(crate) gateway: Ipv4Address,
    pub(crate) dns_server: Ipv4Address,
}

impl InterfaceRuntimeState {
    pub(crate) fn pending_dynamic() -> Self {
        Self {
            mode: NetworkConfigMode::Dynamic,
            state: NetworkConfigState::Pending,
            address: Ipv4Address::UNSPECIFIED,
            prefix_len: 0,
            gateway: Ipv4Address::UNSPECIFIED,
            dns_server: Ipv4Address::UNSPECIFIED,
        }
    }

    pub(crate) fn static_config(config: NetworkConfig) -> Self {
        Self {
            mode: if config.dynamic_ipv4 {
                NetworkConfigMode::Dynamic
            } else {
                NetworkConfigMode::Static
            },
            state: if config.dynamic_ipv4 {
                NetworkConfigState::FallbackStatic
            } else {
                NetworkConfigState::Configured
            },
            address: config.static_address,
            prefix_len: config.static_prefix_len,
            gateway: config.static_gateway,
            dns_server: config.dns_server,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct TcpTransportSlot {
    pub(crate) active: bool,
    pub(crate) control_handle: rt::Handle,
    pub(crate) socket_handle: Option<SocketHandle>,
    pub(crate) state: NetworkSocketState,
    pub(crate) remote_address: Ipv4Address,
    pub(crate) remote_port: u16,
    pub(crate) local_port: u16,
    pub(crate) rx_bytes: u64,
    pub(crate) tx_bytes: u64,
    pub(crate) opened_at_ticks: u64,
    pub(crate) last_activity_ticks: u64,
}

impl TcpTransportSlot {
    pub(crate) const fn empty() -> Self {
        Self {
            active: false,
            control_handle: rt::INVALID_HANDLE,
            socket_handle: None,
            state: NetworkSocketState::Closed,
            remote_address: Ipv4Address::UNSPECIFIED,
            remote_port: 0,
            local_port: 0,
            rx_bytes: 0,
            tx_bytes: 0,
            opened_at_ticks: 0,
            last_activity_ticks: 0,
        }
    }
}
