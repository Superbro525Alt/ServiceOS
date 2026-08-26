use smoltcp::wire::Ipv4Address;

use rt::ConfigKey;
use serviceos_userspace_runtime as rt;

use crate::{
    consts::{DEFAULT_HOSTNAME, MAX_HOSTNAME_BYTES, MAX_HOSTS, MAX_HOSTS_RESOURCE_BYTES},
    types::{HostEntry, NetworkConfig},
    util::u32_to_ipv4,
};

/// Options carried as optional `key=value` lines inside the hosts resource
/// file (the network service's boot configuration). Absent lines keep the
/// defaults: built-in hostname, mDNS-LITE responder on, discovery beacon on.
#[derive(Clone, Copy)]
pub(crate) struct NetFileOptions {
    pub(crate) hostname_len: usize,
    pub(crate) hostname: [u8; MAX_HOSTNAME_BYTES],
    pub(crate) mdns_enabled: bool,
    pub(crate) discovery_enabled: bool,
    /// Shared RX ring negotiation (S7 zero-copy path). On by default; the
    /// `rx-ring=off` hosts-file line forces the legacy copied-frame path.
    pub(crate) rx_ring_enabled: bool,
}

impl NetFileOptions {
    pub(crate) fn defaults() -> Self {
        Self {
            hostname_len: DEFAULT_HOSTNAME.len(),
            hostname: {
                let mut name = [0u8; MAX_HOSTNAME_BYTES];
                name[..DEFAULT_HOSTNAME.len()].copy_from_slice(DEFAULT_HOSTNAME.as_bytes());
                name
            },
            mdns_enabled: true,
            discovery_enabled: true,
            rx_ring_enabled: true,
        }
    }
}

/// A valid hostname label: ASCII letters/digits/hyphen, no dots.
fn is_hostname_label(name: &[u8]) -> bool {
    !name.is_empty()
        && name.len() <= MAX_HOSTNAME_BYTES
        && name
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
}

pub(crate) fn read_network_config(config_handle: rt::Handle) -> rt::Result<NetworkConfig> {
    Ok(NetworkConfig {
        static_address: u32_to_ipv4(read_config_value(
            config_handle,
            ConfigKey::NetworkIpv4Address,
            0,
        )? as u32),
        static_prefix_len: read_config_value(config_handle, ConfigKey::NetworkIpv4PrefixLength, 24)?
            as u8,
        static_gateway: u32_to_ipv4(read_config_value(
            config_handle,
            ConfigKey::NetworkIpv4Gateway,
            0,
        )? as u32),
        dynamic_ipv4: read_config_value(config_handle, ConfigKey::NetworkDynamicIpv4, 0)? != 0,
        dns_server: u32_to_ipv4(
            read_config_value(config_handle, ConfigKey::NetworkDnsServer, 0)? as u32,
        ),
        probe_timeout_ticks: read_config_value(
            config_handle,
            ConfigKey::NetworkProbeTimeoutTicks,
            300,
        )?,
        dns_query_timeout_ticks: read_config_value(
            config_handle,
            ConfigKey::NetworkDnsQueryTimeoutTicks,
            400,
        )?,
        dhcp_acquire_timeout_ticks: read_config_value(
            config_handle,
            ConfigKey::NetworkDhcpAcquireTimeoutTicks,
            600,
        )?,
        tcp_connect_timeout_ticks: read_config_value(
            config_handle,
            ConfigKey::NetworkTcpConnectTimeoutTicks,
            600,
        )?,
        tcp_idle_timeout_ticks: read_config_value(
            config_handle,
            ConfigKey::NetworkTcpIdleTimeoutTicks,
            300,
        )?,
    })
}

fn read_config_value(handle: rt::Handle, key: ConfigKey, default: u64) -> rt::Result<u64> {
    match rt::config_read(handle, key) {
        Ok((_, value)) => Ok(value),
        Err(rt::Error::InvalidArgument) => Ok(default),
        Err(error) => Err(error),
    }
}

pub(crate) fn load_hosts(
    handle: rt::Handle,
    hosts: &mut [HostEntry; MAX_HOSTS],
    options: &mut NetFileOptions,
) -> rt::Result<usize> {
    if handle == rt::INVALID_HANDLE {
        return Ok(0);
    }

    let mut buffer = [0u8; MAX_HOSTS_RESOURCE_BYTES];
    let expected_len = buffer.len();
    let loaded = rt::storage_read_all(handle, &mut buffer, expected_len).unwrap_or(0);
    let _ = rt::storage_blob_close(handle);
    let text = core::str::from_utf8(&buffer[..loaded]).map_err(|_| rt::Error::InvalidArgument)?;
    let mut count = 0usize;

    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        let name = name.trim();
        let value = value.trim();

        // Option lines (parsed but not host-table entries).
        match name {
            "hostname" => {
                if is_hostname_label(value.as_bytes()) {
                    options.hostname_len = value.len();
                    options.hostname[..value.len()].copy_from_slice(value.as_bytes());
                }
                continue;
            }
            "mdns" => {
                options.mdns_enabled = value != "off" && value != "0";
                continue;
            }
            "discovery" => {
                options.discovery_enabled = value != "off" && value != "0";
                continue;
            }
            "rx-ring" => {
                options.rx_ring_enabled = value != "off" && value != "0";
                continue;
            }
            _ => {}
        }

        if count == hosts.len() {
            break;
        }
        let Some(address) = parse_ipv4(value) else {
            continue;
        };
        let name_bytes = name.as_bytes();
        if name_bytes.len() > MAX_HOSTNAME_BYTES {
            continue;
        }
        hosts[count].name_len = name_bytes.len();
        hosts[count].name[..name_bytes.len()].copy_from_slice(name_bytes);
        hosts[count].address = address;
        count += 1;
    }

    Ok(count)
}

pub(crate) fn parse_ipv4(value: &str) -> Option<Ipv4Address> {
    let mut octets = [0u8; 4];
    let mut count = 0usize;
    for part in value.split('.') {
        if count == 4 {
            return None;
        }
        let byte = part.parse::<u8>().ok()?;
        octets[count] = byte;
        count += 1;
    }
    if count == 4 {
        Some(Ipv4Address::new(octets[0], octets[1], octets[2], octets[3]))
    } else {
        None
    }
}
