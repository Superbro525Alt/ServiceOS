use smoltcp::wire::Ipv4Address;

use serviceos_userspace_runtime as rt;
use rt::ConfigKey;

use crate::{
    consts::{MAX_HOSTNAME_BYTES, MAX_HOSTS, MAX_HOSTS_RESOURCE_BYTES},
    types::{HostEntry, NetworkConfig},
    util::u32_to_ipv4,
};

pub(crate) fn read_network_config(config_handle: rt::Handle) -> rt::Result<NetworkConfig> {
    Ok(NetworkConfig {
        static_address: u32_to_ipv4(
            read_config_value(config_handle, ConfigKey::NetworkIpv4Address, 0)? as u32,
        ),
        static_prefix_len: read_config_value(
            config_handle,
            ConfigKey::NetworkIpv4PrefixLength,
            24,
        )? as u8,
        static_gateway: u32_to_ipv4(
            read_config_value(config_handle, ConfigKey::NetworkIpv4Gateway, 0)? as u32,
        ),
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

pub(crate) fn load_hosts(handle: rt::Handle, hosts: &mut [HostEntry; MAX_HOSTS]) -> rt::Result<usize> {
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
        if count == hosts.len() {
            break;
        }
        let Some(address) = parse_ipv4(value.trim()) else {
            continue;
        };
        let name = name.trim().as_bytes();
        if name.len() > MAX_HOSTNAME_BYTES {
            continue;
        }
        hosts[count].name_len = name.len();
        hosts[count].name[..name.len()].copy_from_slice(name);
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
        Some(Ipv4Address::new(
            octets[0], octets[1], octets[2], octets[3],
        ))
    } else {
        None
    }
}
