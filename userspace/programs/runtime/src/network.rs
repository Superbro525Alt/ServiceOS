use crate::{
    Error, Handle, IPC_MAX_WORDS, NetworkDiagPingStats, NetworkDiscoveryPeer,
    NetworkFirewallSummary, NetworkInterfaceStatusInfo, NetworkListenPort, NetworkNeighborEntry,
    NetworkSocketInfo, NetworkSocketKind, NetworkSocketTag, NetworkStatus, NetworkTag, RawMessage,
    Result, channel_call, network_config_mode_from_word, network_config_state_from_word,
    network_listen_port_kind_from_word, network_socket_kind_from_word,
    network_socket_state_from_word, network_status_error, network_status_from_word, pack_bytes,
    packet_backend_from_word, packet_link_state_from_word, unpack_bytes, unpack_mac,
};

pub fn network_interface_count(network_handle: Handle) -> Result<usize> {
    let mut request = RawMessage::empty(NetworkTag::InterfaceListRequest as u32);
    let response = channel_call(network_handle, &mut request)?;
    if response.tag != NetworkTag::InterfaceListReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    match network_status_from_word(response.words[0]) {
        NetworkStatus::Ok => Ok(response.words[1] as usize),
        NetworkStatus::Busy => Err(Error::Busy),
        NetworkStatus::Unsupported => Err(Error::Unsupported),
        _ => Err(Error::InvalidArgument),
    }
}

pub fn network_interface_status(
    network_handle: Handle,
    index: usize,
) -> Result<Option<NetworkInterfaceStatusInfo>> {
    let mut request = RawMessage::empty(NetworkTag::InterfaceStatusRequest as u32);
    request.word_count = 1;
    request.words[0] = index as u64;
    let response = channel_call(network_handle, &mut request)?;
    network_interface_status_parse_reply(&response)
}

pub(crate) fn network_interface_status_parse_reply(
    response: &RawMessage,
) -> Result<Option<NetworkInterfaceStatusInfo>> {
    if response.tag != NetworkTag::InterfaceStatusReply as u32 || response.word_count < 15 {
        return Err(Error::InvalidArgument);
    }

    let status = network_status_from_word(response.words[0]);
    if status == NetworkStatus::NotFound {
        return Ok(None);
    }
    if status != NetworkStatus::Ok {
        return Err(network_status_error(status));
    }

    Ok(Some(NetworkInterfaceStatusInfo {
        index: response.words[1] as u32,
        backend: packet_backend_from_word(response.words[2]),
        link_state: packet_link_state_from_word(response.words[3]),
        mtu: response.words[4] as u32,
        config_mode: network_config_mode_from_word(response.words[5]),
        config_state: network_config_state_from_word(response.words[6]),
        address: response.words[7] as u32,
        prefix_len: response.words[8] as u8,
        gateway: response.words[9] as u32,
        dns_server: response.words[10] as u32,
        mac: unpack_mac(response.words[11]),
        rx_packets: response.words[12],
        tx_packets: response.words[13],
        dropped_packets: response.words[14],
        resolver_hits: if response.word_count >= 16 {
            (response.words[15] >> 32) as u32
        } else {
            0
        },
        resolver_misses: if response.word_count >= 16 {
            (response.words[15] & 0xffff_ffff) as u32
        } else {
            0
        },
    }))
}

/// Longest inline name/target the network-service decodes into its
/// MAX_HOSTNAME_BYTES staging buffer; keeping requests inside this bound
/// guarantees a reply even on decode failure.
pub const NETWORK_NAME_BYTES_MAX: usize = 48;

/// Probe budget per DiagPingStatsRequest; mirrors the service-side clamp.
pub const NETWORK_DIAG_PINGS_MAX: usize = 8;

pub fn network_hostname_get(network_handle: Handle, name: &mut [u8]) -> Result<usize> {
    let response = channel_call(
        network_handle,
        &mut RawMessage::empty(NetworkTag::HostnameGetRequest as u32),
    )?;
    network_hostname_parse_reply(&response, name)
}

pub fn network_hostname_parse_reply(response: &RawMessage, name: &mut [u8]) -> Result<usize> {
    if response.tag != NetworkTag::HostnameGetReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    let status = network_status_from_word(response.words[0]);
    if status != NetworkStatus::Ok {
        return Err(network_status_error(status));
    }
    let len = response.words[1] as usize;
    if len > name.len() {
        return Err(Error::BufferTooSmall);
    }
    unpack_bytes(&response.words[2..response.word_count as usize], len, name)?;
    Ok(len)
}

pub fn network_hostname_set(network_handle: Handle, name: &str) -> Result<()> {
    let mut request = network_hostname_set_request(name)?;
    let response = channel_call(network_handle, &mut request)?;
    if response.tag != NetworkTag::HostnameSetReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    let status = network_status_from_word(response.words[0]);
    if status != NetworkStatus::Ok {
        return Err(network_status_error(status));
    }
    Ok(())
}

pub fn network_hostname_set_request(name: &str) -> Result<RawMessage> {
    let name_bytes = name.as_bytes();
    if name_bytes.len() > NETWORK_NAME_BYTES_MAX {
        return Err(Error::BufferTooSmall);
    }
    let mut request = RawMessage::empty(NetworkTag::HostnameSetRequest as u32);
    request.word_count = 1 + pack_bytes(name_bytes, &mut request.words[1..])?;
    request.words[0] = name_bytes.len() as u64;
    Ok(request)
}

pub fn network_diag_ping_stats(
    network_handle: Handle,
    target: &str,
    count: usize,
) -> Result<NetworkDiagPingStats> {
    let mut request = network_diag_ping_stats_request(target, count)?;
    let response = channel_call(network_handle, &mut request)?;
    network_diag_ping_stats_parse_reply(&response)
}

pub fn network_diag_ping_stats_request(target: &str, count: usize) -> Result<RawMessage> {
    let target_bytes = target.as_bytes();
    if target_bytes.len() > NETWORK_NAME_BYTES_MAX {
        return Err(Error::BufferTooSmall);
    }
    let mut request = RawMessage::empty(NetworkTag::DiagPingStatsRequest as u32);
    request.word_count = 2 + pack_bytes(target_bytes, &mut request.words[2..])?;
    request.words[0] = target_bytes.len() as u64;
    request.words[1] = count.clamp(1, NETWORK_DIAG_PINGS_MAX) as u64;
    Ok(request)
}

pub fn network_diag_ping_stats_parse_reply(response: &RawMessage) -> Result<NetworkDiagPingStats> {
    if response.tag != NetworkTag::DiagPingStatsReply as u32 || response.word_count < 9 {
        return Err(Error::InvalidArgument);
    }
    let status = network_status_from_word(response.words[0]);
    // Timeout means every probe timed out; the stats words still carry the
    // honest zero-received summary and permil loss.
    if status != NetworkStatus::Ok && status != NetworkStatus::Timeout {
        return Err(network_status_error(status));
    }
    Ok(NetworkDiagPingStats {
        resolved_address: response.words[1] as u32,
        sent: response.words[2] as u32,
        received: response.words[3] as u32,
        min_ms: response.words[4],
        max_ms: response.words[5],
        avg_ms: response.words[6],
        jitter_ms: response.words[7],
        loss_permil: response.words[8],
    })
}

pub fn network_neighbor_list(
    network_handle: Handle,
    neighbors: &mut [NetworkNeighborEntry],
) -> Result<usize> {
    let response = channel_call(
        network_handle,
        &mut RawMessage::empty(NetworkTag::NeighborDumpRequest as u32),
    )?;
    network_neighbor_parse_reply(&response, neighbors)
}

pub fn network_neighbor_parse_reply(
    response: &RawMessage,
    neighbors: &mut [NetworkNeighborEntry],
) -> Result<usize> {
    if response.tag != NetworkTag::NeighborDumpReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    let status = network_status_from_word(response.words[0]);
    if status != NetworkStatus::Ok {
        return Err(network_status_error(status));
    }
    let count = response.words[1] as usize;
    if count > neighbors.len() || response.word_count as usize != 2 + count * 2 {
        return Err(Error::BufferTooSmall);
    }
    for (index, neighbor) in neighbors.iter_mut().enumerate().take(count) {
        let base = 2 + index * 2;
        *neighbor = NetworkNeighborEntry {
            address: response.words[base] as u32,
            mac: unpack_mac(response.words[base + 1]),
        };
    }
    Ok(count)
}

pub fn network_listen_ports(
    network_handle: Handle,
    ports: &mut [NetworkListenPort],
) -> Result<usize> {
    let response = channel_call(
        network_handle,
        &mut RawMessage::empty(NetworkTag::ListenPortsRequest as u32),
    )?;
    network_listen_ports_parse_reply(&response, ports)
}

pub fn network_listen_ports_parse_reply(
    response: &RawMessage,
    ports: &mut [NetworkListenPort],
) -> Result<usize> {
    if response.tag != NetworkTag::ListenPortsReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    let status = network_status_from_word(response.words[0]);
    if status != NetworkStatus::Ok {
        return Err(network_status_error(status));
    }
    let count = response.words[1] as usize;
    if count > ports.len() || response.word_count as usize != 2 + count {
        return Err(Error::BufferTooSmall);
    }
    for (index, port) in ports.iter_mut().enumerate().take(count) {
        let word = response.words[2 + index];
        *port = NetworkListenPort {
            kind: network_listen_port_kind_from_word(word >> 48),
            port: (word & 0xffff) as u16,
        };
    }
    Ok(count)
}

pub fn network_discovery_peers(
    network_handle: Handle,
    window_ms: u64,
    peers: &mut [NetworkDiscoveryPeer],
) -> Result<usize> {
    let mut request = RawMessage::empty(NetworkTag::DiscoveryPeersRequest as u32);
    request.word_count = 1;
    request.words[0] = window_ms;
    let response = channel_call(network_handle, &mut request)?;
    network_discovery_peers_parse_reply(&response, peers)
}

pub fn network_discovery_peers_parse_reply(
    response: &RawMessage,
    peers: &mut [NetworkDiscoveryPeer],
) -> Result<usize> {
    if response.tag != NetworkTag::DiscoveryPeersReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    let status = network_status_from_word(response.words[0]);
    if status != NetworkStatus::Ok {
        return Err(network_status_error(status));
    }
    let count = response.words[1] as usize;
    if count > peers.len() || response.word_count as usize != 2 + count * 3 {
        return Err(Error::BufferTooSmall);
    }
    for (index, peer) in peers.iter_mut().enumerate().take(count) {
        let base = 2 + index * 3;
        let word = response.words[base];
        let name_len = ((word >> 24) & 0xff) as usize;
        if name_len > 15 {
            return Err(Error::InvalidArgument);
        }
        let mut name = [0u8; 15];
        name[..8].copy_from_slice(&response.words[base + 1].to_le_bytes());
        name[8..].copy_from_slice(&response.words[base + 2].to_le_bytes()[..7]);
        *peer = NetworkDiscoveryPeer {
            address: (word >> 32) as u32,
            name_len,
            name,
            age_ms: (word & 0xff_ffff) as u32,
        };
    }
    Ok(count)
}

pub fn network_firewall_summary(network_handle: Handle) -> Result<NetworkFirewallSummary> {
    let response = channel_call(
        network_handle,
        &mut RawMessage::empty(NetworkTag::FirewallRulesGetRequest as u32),
    )?;
    network_firewall_summary_parse_reply(&response)
}

pub fn network_firewall_summary_parse_reply(
    response: &RawMessage,
) -> Result<NetworkFirewallSummary> {
    if response.tag != NetworkTag::FirewallRulesReply as u32 || response.word_count < 4 {
        return Err(Error::InvalidArgument);
    }
    let status = network_status_from_word(response.words[0]);
    if status != NetworkStatus::Ok {
        return Err(network_status_error(status));
    }
    Ok(NetworkFirewallSummary {
        rule_count: response.words[1] as u32,
        default_inbound_allow: response.words[2] != 0,
        inbound_denied_total: (response.words[3] >> 32) as u32,
        outbound_denied_total: (response.words[3] & 0xffff_ffff) as u32,
    })
}

pub fn network_resolve(network_handle: Handle, name: &str, addresses: &mut [u32]) -> Result<usize> {
    let name_bytes = name.as_bytes();
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(1)) * 8;
    if name_bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let mut request = RawMessage::empty(NetworkTag::ResolveRequest as u32);
    request.word_count = 1 + pack_bytes(name_bytes, &mut request.words[1..])?;
    request.words[0] = name_bytes.len() as u64;
    let response = channel_call(network_handle, &mut request)?;
    if response.tag != NetworkTag::ResolveReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }

    let status = network_status_from_word(response.words[0]);
    if status != NetworkStatus::Ok {
        return Err(network_status_error(status));
    }

    let count = response.words[1] as usize;
    if count > addresses.len() || (response.word_count as usize) < 2 + count {
        return Err(Error::BufferTooSmall);
    }
    for (index, address) in addresses.iter_mut().enumerate().take(count) {
        *address = response.words[2 + index] as u32;
    }
    Ok(count)
}

pub fn network_ping(network_handle: Handle, target: &str) -> Result<(u32, u64)> {
    let target_bytes = target.as_bytes();
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(1)) * 8;
    if target_bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let mut request = RawMessage::empty(NetworkTag::PingRequest as u32);
    request.word_count = 1 + pack_bytes(target_bytes, &mut request.words[1..])?;
    request.words[0] = target_bytes.len() as u64;
    let response = channel_call(network_handle, &mut request)?;
    if response.tag != NetworkTag::PingReply as u32 || response.word_count < 3 {
        return Err(Error::InvalidArgument);
    }

    let status = network_status_from_word(response.words[0]);
    if status != NetworkStatus::Ok {
        return Err(network_status_error(status));
    }

    Ok((response.words[1] as u32, response.words[2]))
}

pub fn network_socket_open(
    network_handle: Handle,
    kind: NetworkSocketKind,
    target: &str,
    port: u16,
) -> Result<Handle> {
    let target_bytes = target.as_bytes();
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(2)) * 8;
    if target_bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let mut request = RawMessage::empty(NetworkTag::SocketOpenRequest as u32);
    request.word_count = 2 + pack_bytes(target_bytes, &mut request.words[2..])?;
    request.words[0] = kind as u32 as u64;
    request.words[1] = ((target_bytes.len() as u64) << 16) | port as u64;
    let response = channel_call(network_handle, &mut request)?;
    if response.tag != NetworkTag::SocketOpenReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }

    let status = network_status_from_word(response.words[0]);
    if status != NetworkStatus::Ok {
        return Err(network_status_error(status));
    }
    if response.handle_count < 1 {
        return Err(Error::InvalidArgument);
    }
    Ok(response.handles[0])
}

pub fn network_socket_list(
    network_handle: Handle,
    sockets: &mut [NetworkSocketInfo],
) -> Result<usize> {
    let mut request = RawMessage::empty(NetworkTag::SocketListRequest as u32);
    let response = channel_call(network_handle, &mut request)?;
    if response.tag != NetworkTag::SocketListReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }

    let status = network_status_from_word(response.words[0]);
    if status != NetworkStatus::Ok {
        return Err(network_status_error(status));
    }

    let count = response.words[1] as usize;
    if count > sockets.len() || response.word_count as usize != 2 + count * 7 {
        return Err(Error::BufferTooSmall);
    }
    for (index, socket) in sockets.iter_mut().enumerate().take(count) {
        let base = 2 + index * 7;
        *socket = NetworkSocketInfo {
            slot: response.words[base] as u32,
            kind: network_socket_kind_from_word(response.words[base + 1]),
            state: network_socket_state_from_word(response.words[base + 2]),
            remote_address: response.words[base + 3] as u32,
            remote_port: response.words[base + 4] as u16,
            local_port: response.words[base + 5] as u16,
            rx_bytes: response.words[base + 6] >> 32,
            tx_bytes: response.words[base + 6] & 0xffff_ffff,
        };
    }
    Ok(count)
}

pub fn network_socket_status(socket_handle: Handle) -> Result<NetworkSocketInfo> {
    let mut request = RawMessage::empty(NetworkSocketTag::StatusRequest as u32);
    let response = channel_call(socket_handle, &mut request)?;
    if response.tag != NetworkSocketTag::StatusReply as u32 || response.word_count < 8 {
        return Err(Error::InvalidArgument);
    }
    let status = network_status_from_word(response.words[0]);
    if status != NetworkStatus::Ok {
        return Err(network_status_error(status));
    }
    Ok(NetworkSocketInfo {
        slot: response.words[1] as u32,
        kind: network_socket_kind_from_word(response.words[2]),
        state: network_socket_state_from_word(response.words[3]),
        remote_address: response.words[4] as u32,
        remote_port: response.words[5] as u16,
        local_port: response.words[6] as u16,
        rx_bytes: response.words[7] >> 32,
        tx_bytes: response.words[7] & 0xffff_ffff,
    })
}

pub fn network_socket_send(socket_handle: Handle, payload: &[u8]) -> Result<usize> {
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(1)) * 8;
    if payload.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let mut request = RawMessage::empty(NetworkSocketTag::SendRequest as u32);
    request.word_count = 1 + pack_bytes(payload, &mut request.words[1..])?;
    request.words[0] = payload.len() as u64;
    let response = channel_call(socket_handle, &mut request)?;
    if response.tag != NetworkSocketTag::SendReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    let status = network_status_from_word(response.words[0]);
    if status != NetworkStatus::Ok {
        return Err(network_status_error(status));
    }
    Ok(response.words[1] as usize)
}

pub fn network_socket_receive(socket_handle: Handle, buffer: &mut [u8]) -> Result<usize> {
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(2)) * 8;
    let requested = buffer.len().min(max_inline_bytes);
    let mut request = RawMessage::empty(NetworkSocketTag::ReceiveRequest as u32);
    request.word_count = 1;
    request.words[0] = requested as u64;
    let response = channel_call(socket_handle, &mut request)?;
    if response.tag != NetworkSocketTag::ReceiveReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    let status = network_status_from_word(response.words[0]);
    if status != NetworkStatus::Ok {
        return Err(network_status_error(status));
    }
    let count = response.words[1] as usize;
    unpack_bytes(
        &response.words[2..response.word_count as usize],
        count,
        buffer,
    )?;
    Ok(count)
}

pub fn network_socket_close(socket_handle: Handle) -> Result<()> {
    let mut request = RawMessage::empty(NetworkSocketTag::CloseRequest as u32);
    let response = channel_call(socket_handle, &mut request)?;
    if response.tag != NetworkSocketTag::CloseReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    let status = network_status_from_word(response.words[0]);
    if status != NetworkStatus::Ok && status != NetworkStatus::Closed {
        return Err(network_status_error(status));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NetworkListenPortKind;

    #[test]
    fn hostname_set_request_packs_name_inline() {
        let request = network_hostname_set_request("serviceos").expect("name within cap packs");
        assert_eq!(request.tag, NetworkTag::HostnameSetRequest as u32);
        assert_eq!(request.words[0], 9);
        let mut name = [0u8; 16];
        unpack_bytes(&request.words[1..3], 9, &mut name).expect("name decodes");
        assert_eq!(&name[..9], b"serviceos");

        let long = [b'a'; NETWORK_NAME_BYTES_MAX + 1];
        assert!(core::str::from_utf8(&long).is_ok());
        assert_eq!(
            network_hostname_set_request(core::str::from_utf8(&long).expect("ascii"))
                .expect_err("over-long name rejected"),
            Error::BufferTooSmall
        );
    }

    #[test]
    fn hostname_get_parse_reply_decodes_name() {
        let mut payload = [0u64; 8];
        let packed = pack_bytes(b"serviceos", &mut payload).expect("name packs");
        let mut reply = RawMessage::empty(NetworkTag::HostnameGetReply as u32);
        reply.word_count = 2 + packed;
        reply.words[0] = NetworkStatus::Ok as u32 as u64;
        reply.words[1] = 9;
        reply.words[2..2 + packed as usize].copy_from_slice(&payload[..packed as usize]);

        let mut name = [0u8; 32];
        let len = network_hostname_parse_reply(&reply, &mut name).expect("reply parses");
        assert_eq!(len, 9);
        assert_eq!(&name[..len], b"serviceos");

        reply.words[0] = NetworkStatus::Busy as u32 as u64;
        assert_eq!(
            network_hostname_parse_reply(&reply, &mut name).expect_err("busy maps to error"),
            Error::Busy
        );

        let mut tiny = [0u8; 4];
        reply.words[0] = NetworkStatus::Ok as u32 as u64;
        assert_eq!(
            network_hostname_parse_reply(&reply, &mut tiny).expect_err("short buffer rejected"),
            Error::BufferTooSmall
        );
    }

    #[test]
    fn diag_ping_stats_request_clamps_count_and_packs_target() {
        let request = network_diag_ping_stats_request("10.0.2.2", 99).expect("request builds");
        assert_eq!(request.tag, NetworkTag::DiagPingStatsRequest as u32);
        assert_eq!(request.words[0], 8);
        assert_eq!(request.words[1], NETWORK_DIAG_PINGS_MAX as u64);
        let mut target = [0u8; 16];
        unpack_bytes(&request.words[2..3], 8, &mut target).expect("target decodes");
        assert_eq!(&target[..8], b"10.0.2.2");

        let request = network_diag_ping_stats_request("gw", 0).expect("request builds");
        assert_eq!(request.words[1], 1, "count clamps up to one probe");
    }

    #[test]
    fn diag_ping_stats_parse_reply_decodes_summary() {
        let mut reply = RawMessage::empty(NetworkTag::DiagPingStatsReply as u32);
        reply.word_count = 9;
        reply.words[0] = NetworkStatus::Ok as u32 as u64;
        reply.words[1] = 0x0a00_0202;
        reply.words[2] = 8;
        reply.words[3] = 7;
        reply.words[4] = 4;
        reply.words[5] = 20;
        reply.words[6] = 11;
        reply.words[7] = 5;
        reply.words[8] = 125;

        let stats = network_diag_ping_stats_parse_reply(&reply).expect("stats parse");
        assert_eq!(stats.resolved_address, 0x0a00_0202);
        assert_eq!(stats.sent, 8);
        assert_eq!(stats.received, 7);
        assert_eq!(stats.min_ms, 4);
        assert_eq!(stats.max_ms, 20);
        assert_eq!(stats.avg_ms, 11);
        assert_eq!(stats.jitter_ms, 5);
        assert_eq!(stats.loss_permil, 125);

        // All probes lost: Timeout still decodes an honest zero summary
        // (the service zeroes the stats words on timeout).
        reply.words[0] = NetworkStatus::Timeout as u32 as u64;
        reply.words[3] = 0;
        reply.words[8] = 1000;
        let stats = network_diag_ping_stats_parse_reply(&reply).expect("timeout parses");
        assert_eq!(stats.received, 0);
        assert_eq!(stats.loss_permil, 1000);

        reply.words[0] = NetworkStatus::Denied as u32 as u64;
        assert_eq!(
            network_diag_ping_stats_parse_reply(&reply).expect_err("denied maps to error"),
            Error::PermissionDenied
        );
    }

    #[test]
    fn neighbor_parse_reply_decodes_address_mac_pairs() {
        let mut reply = RawMessage::empty(NetworkTag::NeighborDumpReply as u32);
        reply.word_count = 4;
        reply.words[0] = NetworkStatus::Ok as u32 as u64;
        reply.words[1] = 1;
        reply.words[2] = 0x0a00_020f;
        reply.words[3] = 0x5254_0012_3456;

        let mut neighbors = [NetworkNeighborEntry {
            address: 0,
            mac: [0; 6],
        }; 6];
        let count = network_neighbor_parse_reply(&reply, &mut neighbors).expect("parses");
        assert_eq!(count, 1);
        assert_eq!(neighbors[0].address, 0x0a00_020f);
        assert_eq!(neighbors[0].mac, [0x56, 0x34, 0x12, 0x00, 0x54, 0x52]);

        reply.word_count = 5;
        assert_eq!(
            network_neighbor_parse_reply(&reply, &mut neighbors)
                .expect_err("ragged reply rejected"),
            Error::BufferTooSmall
        );
    }

    #[test]
    fn listen_ports_parse_reply_decodes_kind_port_words() {
        let mut reply = RawMessage::empty(NetworkTag::ListenPortsReply as u32);
        reply.word_count = 4;
        reply.words[0] = NetworkStatus::Ok as u32 as u64;
        reply.words[1] = 2;
        reply.words[2] = (1u64 << 48) | 80;
        reply.words[3] = (3u64 << 48) | 41453;

        let mut ports = [NetworkListenPort {
            kind: NetworkListenPortKind::Unknown,
            port: 0,
        }; 8];
        let count = network_listen_ports_parse_reply(&reply, &mut ports).expect("parses");
        assert_eq!(count, 2);
        assert_eq!(ports[0].kind, NetworkListenPortKind::TcpListener);
        assert_eq!(ports[0].port, 80);
        assert_eq!(ports[1].kind, NetworkListenPortKind::UdpInternal);
        assert_eq!(ports[1].port, 41453);

        let mut none = [];
        let _ = network_listen_ports_parse_reply(&reply, &mut none)
            .expect_err("reply exceeding buffer rejected");
    }

    #[test]
    fn discovery_peers_parse_reply_decodes_packed_peer_words() {
        let mut name1 = [0u8; 15];
        name1[..6].copy_from_slice(b"host-1");
        let mut w1 = [0u64; 2];
        let _ = pack_bytes(&name1[..8], &mut w1);
        let mut w2 = [0u64; 1];
        let _ = pack_bytes(&name1[8..], &mut w2);

        let mut reply = RawMessage::empty(NetworkTag::DiscoveryPeersReply as u32);
        reply.word_count = 5;
        reply.words[0] = NetworkStatus::Ok as u32 as u64;
        reply.words[1] = 1;
        reply.words[2] = (0x0a00_0209u64 << 32) | (6u64 << 24) | 1234;
        reply.words[3] = w1[0];
        reply.words[4] = w2[0];

        let mut peers = [NetworkDiscoveryPeer {
            address: 0,
            name_len: 0,
            name: [0; 15],
            age_ms: 0,
        }; 4];
        let count = network_discovery_peers_parse_reply(&reply, &mut peers).expect("parses");
        assert_eq!(count, 1);
        assert_eq!(peers[0].address, 0x0a00_0209);
        assert_eq!(peers[0].name_len, 6);
        assert_eq!(&peers[0].name[..6], b"host-1");
        assert_eq!(peers[0].age_ms, 1234);
    }

    #[test]
    fn firewall_summary_parse_reply_decodes_policy_and_counters() {
        let mut reply = RawMessage::empty(NetworkTag::FirewallRulesReply as u32);
        reply.word_count = 4;
        reply.words[0] = NetworkStatus::Ok as u32 as u64;
        reply.words[1] = 3;
        reply.words[2] = 0;
        reply.words[3] = (7u64 << 32) | 2;

        let summary = network_firewall_summary_parse_reply(&reply).expect("parses");
        assert_eq!(summary.rule_count, 3);
        assert!(!summary.default_inbound_allow);
        assert_eq!(summary.inbound_denied_total, 7);
        assert_eq!(summary.outbound_denied_total, 2);

        reply.words[0] = NetworkStatus::Unsupported as u32 as u64;
        assert_eq!(
            network_firewall_summary_parse_reply(&reply).expect_err("unsupported maps to error"),
            Error::Unsupported
        );
    }

    #[test]
    fn interface_status_reply_trailing_words_are_optional() {
        // A 15-word legacy reply still decodes; counters read zero.
        let mut reply = RawMessage::empty(NetworkTag::InterfaceStatusReply as u32);
        reply.word_count = 15;
        reply.words[0] = NetworkStatus::Ok as u32 as u64;
        let parsed = network_interface_status_parse_reply(&reply).expect("parses");
        let info = parsed.expect("status present");
        assert_eq!(info.resolver_hits, 0);
        assert_eq!(info.resolver_misses, 0);

        reply.word_count = 16;
        reply.words[15] = (12u64 << 32) | 34;
        let info = network_interface_status_parse_reply(&reply)
            .expect("parses")
            .expect("status");
        assert_eq!(info.resolver_hits, 12);
        assert_eq!(info.resolver_misses, 34);
    }
}
