use crate::{
    Error, Handle, IPC_MAX_WORDS, NetworkDiagPingStats, NetworkDiscoveryPeer,
    NetworkFirewallSummary, NetworkInterfaceStatusInfo, NetworkListenPort, NetworkNeighborEntry,
    NetworkSocketInfo, NetworkSocketKind, NetworkSocketTag, NetworkStatus, NetworkTag,
    NetworkWifiSavedNetwork, NetworkWifiScanEntry, NetworkWifiStatus, RawMessage, Result,
    WifiLinkState, WifiSecurity, channel_call, network_config_mode_from_word,
    network_config_state_from_word, network_listen_port_kind_from_word,
    network_socket_kind_from_word, network_socket_state_from_word, network_status_error,
    network_status_from_word, pack_bytes, packet_backend_from_word, packet_link_state_from_word,
    unpack_bytes, unpack_mac,
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

// --- Wireless control (network-service Wi-Fi family) ---

/// Longest SSID/PSK the runtime will inline into a wireless request; mirrors
/// the 802.11 limits enforced by the pure layer.
pub const NETWORK_WIFI_SSID_BYTES_MAX: usize = 32;
pub const NETWORK_WIFI_PSK_BYTES_MAX: usize = 64;

/// Entry capacity a caller must provide to always receive a full
/// WifiScanReply message (the service caps replies at 2 entries).
pub const NETWORK_WIFI_SCAN_REPLY_ENTRIES_MAX: usize = 2;
/// Entry capacity matching a full WifiSavedListReply message.
pub const NETWORK_WIFI_SAVED_REPLY_ENTRIES_MAX: usize = 2;

/// Words per packed scan entry in a WifiScanReply (see the ABI docs).
pub(crate) const WIFI_SCAN_ENTRY_WORDS: usize = 6;
/// Words per packed saved-network entry in a WifiSavedListReply.
pub(crate) const WIFI_SAVED_ENTRY_WORDS: usize = 5;

fn wifi_link_state_from_word(word: u64) -> Result<WifiLinkState> {
    match word {
        x if x == WifiLinkState::Down as u64 => Ok(WifiLinkState::Down),
        x if x == WifiLinkState::Scanning as u64 => Ok(WifiLinkState::Scanning),
        x if x == WifiLinkState::Authenticating as u64 => Ok(WifiLinkState::Authenticating),
        x if x == WifiLinkState::Associating as u64 => Ok(WifiLinkState::Associating),
        x if x == WifiLinkState::Connected as u64 => Ok(WifiLinkState::Connected),
        _ => Err(Error::InvalidArgument),
    }
}

pub fn network_wifi_security_from_word(word: u64) -> Result<WifiSecurity> {
    match word {
        x if x == WifiSecurity::Open as u64 => Ok(WifiSecurity::Open),
        x if x == WifiSecurity::Wpa2 as u64 => Ok(WifiSecurity::Wpa2),
        x if x == WifiSecurity::Wpa3 as u64 => Ok(WifiSecurity::Wpa3),
        x if x == WifiSecurity::Unknown as u64 => Ok(WifiSecurity::Unknown),
        _ => Err(Error::InvalidArgument),
    }
}

/// Triggers a scan and decodes the entries carried in the reply. With no
/// wireless backend registered this fails with [`Error::Unsupported`] —
/// honest absence, never fabricated results.
pub fn network_wifi_scan(
    network_handle: Handle,
    entries: &mut [NetworkWifiScanEntry],
) -> Result<usize> {
    let response = channel_call(
        network_handle,
        &mut RawMessage::empty(NetworkTag::WifiScanRequest as u32),
    )?;
    network_wifi_scan_parse_reply(&response, entries)
}

pub fn network_wifi_scan_parse_reply(
    response: &RawMessage,
    entries: &mut [NetworkWifiScanEntry],
) -> Result<usize> {
    if response.tag != NetworkTag::WifiScanReply as u32 || response.word_count < 3 {
        return Err(Error::InvalidArgument);
    }
    let status = network_status_from_word(response.words[0]);
    if status != NetworkStatus::Ok {
        return Err(network_status_error(status));
    }
    let total = response.words[1] as usize;
    let carried = response.words[2] as usize;
    if (response.word_count as usize) != 3 + carried * WIFI_SCAN_ENTRY_WORDS {
        return Err(Error::InvalidArgument);
    }
    if carried > entries.len() {
        return Err(Error::BufferTooSmall);
    }
    for (index, entry) in entries.iter_mut().enumerate().take(carried) {
        let base = 3 + index * WIFI_SCAN_ENTRY_WORDS;
        let word0 = response.words[base];
        let word1 = response.words[base + 1];
        let bssid48 = word0 & 0xffff_ffff_ffff;
        let ssid_len = (word1 >> 56) as usize;
        if ssid_len > NETWORK_WIFI_SSID_BYTES_MAX {
            return Err(Error::InvalidArgument);
        }
        let mut ssid = [0u8; NETWORK_WIFI_SSID_BYTES_MAX];
        unpack_bytes(
            &response.words[base + 1..base + WIFI_SCAN_ENTRY_WORDS],
            ssid_len,
            &mut ssid,
        )?;
        *entry = NetworkWifiScanEntry {
            bssid: [
                (bssid48 >> 40) as u8,
                (bssid48 >> 32) as u8,
                (bssid48 >> 24) as u8,
                (bssid48 >> 16) as u8,
                (bssid48 >> 8) as u8,
                bssid48 as u8,
            ],
            channel: (word0 >> 56) as u8,
            rssi: ((word0 >> 48) & 0xff) as u8 as i8,
            ssid_len,
            ssid,
            security: network_wifi_security_from_word((word1 >> 48) & 0xff)?,
        };
    }
    Ok(total)
}

/// Joins a network (None psk = open). With no wireless backend this fails
/// with [`Error::Unsupported`].
pub fn network_wifi_join(
    network_handle: Handle,
    ssid: &str,
    psk: Option<&str>,
) -> Result<WifiLinkState> {
    let mut request = network_wifi_join_request(ssid, psk)?;
    let response = channel_call(network_handle, &mut request)?;
    network_wifi_join_parse_reply(&response)
}

pub fn network_wifi_join_request(ssid: &str, psk: Option<&str>) -> Result<RawMessage> {
    let ssid_bytes = ssid.as_bytes();
    let psk_bytes = psk.unwrap_or("").as_bytes();
    if ssid_bytes.is_empty()
        || ssid_bytes.len() > NETWORK_WIFI_SSID_BYTES_MAX
        || psk_bytes.len() > NETWORK_WIFI_PSK_BYTES_MAX
        || (!psk_bytes.is_empty() && psk_bytes.len() < 8)
    {
        return Err(Error::InvalidArgument);
    }
    let mut request = RawMessage::empty(NetworkTag::WifiJoinRequest as u32);
    request.word_count = 2 + pack_bytes(ssid_bytes, &mut request.words[2..])?;
    let psk_words = pack_bytes(psk_bytes, &mut request.words[request.word_count as usize..])?;
    request.words[0] = ssid_bytes.len() as u64;
    request.words[1] = psk_bytes.len() as u64;
    request.word_count += psk_words;
    Ok(request)
}

pub fn network_wifi_join_parse_reply(response: &RawMessage) -> Result<WifiLinkState> {
    if response.tag != NetworkTag::WifiJoinReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    let status = network_status_from_word(response.words[0]);
    if status != NetworkStatus::Ok {
        return Err(network_status_error(status));
    }
    wifi_link_state_from_word(response.words[1])
}

/// Drops the current wireless link (no-op refusal without a backend).
pub fn network_wifi_leave(network_handle: Handle) -> Result<WifiLinkState> {
    let response = channel_call(
        network_handle,
        &mut RawMessage::empty(NetworkTag::WifiLeaveRequest as u32),
    )?;
    network_wifi_leave_parse_reply(&response)
}

pub fn network_wifi_leave_parse_reply(response: &RawMessage) -> Result<WifiLinkState> {
    if response.tag != NetworkTag::WifiLeaveReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    let status = network_status_from_word(response.words[0]);
    if status != NetworkStatus::Ok {
        return Err(network_status_error(status));
    }
    wifi_link_state_from_word(response.words[1])
}

/// Lists saved networks (up to the reply's per-message entry cap).
pub fn network_wifi_saved_list(
    network_handle: Handle,
    saved: &mut [NetworkWifiSavedNetwork],
) -> Result<usize> {
    let response = channel_call(
        network_handle,
        &mut RawMessage::empty(NetworkTag::WifiSavedListRequest as u32),
    )?;
    network_wifi_saved_list_parse_reply(&response, saved)
}

pub fn network_wifi_saved_list_parse_reply(
    response: &RawMessage,
    saved: &mut [NetworkWifiSavedNetwork],
) -> Result<usize> {
    if response.tag != NetworkTag::WifiSavedListReply as u32 || response.word_count < 3 {
        return Err(Error::InvalidArgument);
    }
    let status = network_status_from_word(response.words[0]);
    if status != NetworkStatus::Ok {
        return Err(network_status_error(status));
    }
    let total = response.words[1] as usize;
    let carried = response.words[2] as usize;
    if (response.word_count as usize) != 3 + carried * WIFI_SAVED_ENTRY_WORDS {
        return Err(Error::InvalidArgument);
    }
    if carried > saved.len() {
        return Err(Error::BufferTooSmall);
    }
    for (index, record) in saved.iter_mut().enumerate().take(carried) {
        let base = 3 + index * WIFI_SAVED_ENTRY_WORDS;
        let word0 = response.words[base];
        let ssid_len = (word0 >> 56) as usize;
        if ssid_len > NETWORK_WIFI_SSID_BYTES_MAX {
            return Err(Error::InvalidArgument);
        }
        let mut ssid = [0u8; NETWORK_WIFI_SSID_BYTES_MAX];
        unpack_bytes(
            &response.words[base..base + WIFI_SAVED_ENTRY_WORDS],
            ssid_len,
            &mut ssid,
        )?;
        *record = NetworkWifiSavedNetwork {
            ssid_len,
            ssid,
            priority: ((word0 >> 48) & 0xff) as u8,
        };
    }
    Ok(total)
}

pub fn network_wifi_saved_add(
    network_handle: Handle,
    ssid: &str,
    psk: &str,
    priority: u8,
) -> Result<()> {
    let mut request = network_wifi_saved_add_request(ssid, psk, priority)?;
    let response = channel_call(network_handle, &mut request)?;
    network_wifi_status_reply(&response, NetworkTag::WifiSavedAddReply)
}

pub fn network_wifi_saved_add_request(ssid: &str, psk: &str, priority: u8) -> Result<RawMessage> {
    let ssid_bytes = ssid.as_bytes();
    let psk_bytes = psk.as_bytes();
    if ssid_bytes.is_empty()
        || ssid_bytes.len() > NETWORK_WIFI_SSID_BYTES_MAX
        || psk_bytes.is_empty()
        || psk_bytes.len() > NETWORK_WIFI_PSK_BYTES_MAX
    {
        return Err(Error::InvalidArgument);
    }
    let mut request = RawMessage::empty(NetworkTag::WifiSavedAddRequest as u32);
    request.word_count = 3 + pack_bytes(ssid_bytes, &mut request.words[3..])?;
    let psk_words = pack_bytes(psk_bytes, &mut request.words[request.word_count as usize..])?;
    request.words[0] = ssid_bytes.len() as u64;
    request.words[1] = psk_bytes.len() as u64;
    request.words[2] = priority as u64;
    request.word_count += psk_words;
    Ok(request)
}

pub fn network_wifi_saved_remove(network_handle: Handle, ssid: &str) -> Result<()> {
    let mut request = network_wifi_saved_remove_request(ssid)?;
    let response = channel_call(network_handle, &mut request)?;
    network_wifi_status_reply(&response, NetworkTag::WifiSavedRemoveReply)
}

pub fn network_wifi_saved_remove_request(ssid: &str) -> Result<RawMessage> {
    let ssid_bytes = ssid.as_bytes();
    if ssid_bytes.is_empty() || ssid_bytes.len() > NETWORK_WIFI_SSID_BYTES_MAX {
        return Err(Error::InvalidArgument);
    }
    let mut request = RawMessage::empty(NetworkTag::WifiSavedRemoveRequest as u32);
    request.word_count = 1 + pack_bytes(ssid_bytes, &mut request.words[1..])?;
    request.words[0] = ssid_bytes.len() as u64;
    Ok(request)
}

/// Wireless status echo. Fails with [`Error::Unsupported`] while no backend
/// exists (the only configuration in-tree today); the parse still validates
/// the echo fields so a future backend reply is shape-checked on day one.
pub fn network_wifi_status(network_handle: Handle) -> Result<NetworkWifiStatus> {
    let response = channel_call(
        network_handle,
        &mut RawMessage::empty(NetworkTag::WifiStatusRequest as u32),
    )?;
    network_wifi_status_parse_reply(&response)
}

pub fn network_wifi_status_parse_reply(response: &RawMessage) -> Result<NetworkWifiStatus> {
    if response.tag != NetworkTag::WifiStatusReply as u32 || response.word_count < 4 {
        return Err(Error::InvalidArgument);
    }
    let status = network_status_from_word(response.words[0]);
    if status != NetworkStatus::Ok {
        return Err(network_status_error(status));
    }
    let ssid_len = response.words[3] as usize;
    if ssid_len > NETWORK_WIFI_SSID_BYTES_MAX {
        return Err(Error::InvalidArgument);
    }
    let mut ssid = [0u8; NETWORK_WIFI_SSID_BYTES_MAX];
    unpack_bytes(&response.words[4..8], ssid_len, &mut ssid)?;
    Ok(NetworkWifiStatus {
        link_state: wifi_link_state_from_word(response.words[1])?,
        backend_present: response.words[2] & 1 != 0,
        ssid_len,
        ssid,
    })
}

fn network_wifi_status_reply(response: &RawMessage, tag: NetworkTag) -> Result<()> {
    if response.tag != tag as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    let status = network_status_from_word(response.words[0]);
    if status != NetworkStatus::Ok {
        return Err(network_status_error(status));
    }
    Ok(())
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

/// Firewall policy surface for the replace-all op of
/// [`NetworkTag::FirewallRulesSetRequest`]. `interface: None` leaves the rule
/// matching every interface (legacy behavior); `Some(index)` pins it to the
/// interface reported by [`NetworkTag::InterfaceStatusRequest`] at that
/// 0-based index (the boot interface is index 0, eth0). The wire encoding
/// rides the existing rule word: bits [48..64) hold `0` for any interface or
/// `index + 1` for a pinned one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkFirewallRule {
    /// `false` = allow, `true` = deny.
    pub deny: bool,
    pub proto: NetworkFirewallProto,
    pub direction: NetworkFirewallDirection,
    /// 0 = any port (inbound compares the local service port, outbound the
    /// remote port).
    pub port: u16,
    pub interface: Option<u16>,
    pub enabled: bool,
}

#[repr(u64)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkFirewallProto {
    Any = 0,
    Tcp = 1,
    Udp = 2,
    Icmp = 3,
}

#[repr(u64)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkFirewallDirection {
    Inbound = 0,
    Outbound = 1,
}

/// Pack a rule into the FirewallRulesSetRequest record word. Mirrors the
/// service-side decode bit-for-bit (action/proto/direction/enabled/port and
/// the trailing interface qualifier).
pub(crate) fn pack_firewall_rule_word(rule: &NetworkFirewallRule) -> u64 {
    (rule.deny as u64)
        | ((rule.proto as u64) << 8)
        | ((rule.direction as u64) << 16)
        | ((rule.enabled as u64) << 24)
        | ((rule.port as u64) << 32)
        | (rule.interface.map_or(0, |index| index as u64 + 1) << 48)
}

/// Build the replace-all request body (FirewallRulesSetRequest op 0). At
/// most 7 rules fit one IPC message (2 header words + 2 record words each);
/// the service side further caps the table at its own maximum and answers
/// [`NetworkStatus::InvalidTarget`] beyond it.
pub(crate) fn firewall_replace_request(rules: &[NetworkFirewallRule]) -> Result<RawMessage> {
    if 2 + rules.len() * 2 > IPC_MAX_WORDS {
        return Err(Error::BufferTooSmall);
    }
    let mut request = RawMessage::empty(NetworkTag::FirewallRulesSetRequest as u32);
    request.word_count = (2 + rules.len() * 2) as u32;
    request.words[0] = 0;
    request.words[1] = rules.len() as u64;
    for (index, rule) in rules.iter().enumerate() {
        let base = 2 + index * 2;
        request.words[base] = pack_firewall_rule_word(rule);
        request.words[base + 1] = rule.enabled as u64;
    }
    Ok(request)
}

/// Replace the whole firewall table (FirewallRulesSetRequest op 0) and
/// return the fresh summary from the FirewallRulesReply.
pub fn network_firewall_rules_replace(
    network_handle: Handle,
    rules: &[NetworkFirewallRule],
) -> Result<NetworkFirewallSummary> {
    let mut request = firewall_replace_request(rules)?;
    let response = channel_call(network_handle, &mut request)?;
    network_firewall_summary_parse_reply(&response)
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

    // --- wireless wrappers ---

    #[test]
    fn wifi_join_request_packs_ssid_psk_inline_and_validates() {
        let request =
            network_wifi_join_request("home", Some("passphrase1")).expect("request builds");
        assert_eq!(request.tag, NetworkTag::WifiJoinRequest as u32);
        assert_eq!(request.words[0], 4);
        assert_eq!(request.words[1], 11);
        assert_eq!(request.word_count, 2 + 1 + 2);
        let mut ssid = [0u8; 8];
        unpack_bytes(&request.words[2..3], 4, &mut ssid).expect("ssid decodes");
        assert_eq!(&ssid[..4], b"home");
        let mut psk = [0u8; 16];
        unpack_bytes(&request.words[3..5], 11, &mut psk).expect("psk decodes");
        assert_eq!(&psk[..11], b"passphrase1");

        // Open network: psk length 0, no psk words.
        let request = network_wifi_join_request("cafe", None).expect("request builds");
        assert_eq!(request.word_count, 3);
        assert_eq!(request.words[1], 0);

        // Validation: empty ssid, oversized psk, too-short psk.
        assert!(network_wifi_join_request("", None).is_err());
        assert!(network_wifi_join_request("home", Some("short")).is_err());
        let long_psk = "p".repeat(NETWORK_WIFI_PSK_BYTES_MAX + 1);
        assert!(network_wifi_join_request("home", Some(&long_psk)).is_err());
        assert!(network_wifi_join_request("", None).is_err());
    }

    #[test]
    fn wifi_join_parse_reply_maps_status_and_link_state() {
        let mut reply = RawMessage::empty(NetworkTag::WifiJoinReply as u32);
        reply.word_count = 2;
        reply.words[0] = NetworkStatus::Ok as u32 as u64;
        reply.words[1] = WifiLinkState::Connected as u64;
        assert_eq!(
            network_wifi_join_parse_reply(&reply).expect("parses"),
            WifiLinkState::Connected
        );

        // Backend absent: Unsupported maps to the honest error.
        reply.words[0] = NetworkStatus::Unsupported as u32 as u64;
        assert_eq!(
            network_wifi_join_parse_reply(&reply).expect_err("unsupported"),
            Error::Unsupported
        );

        // Unknown link-state word is rejected.
        reply.words[0] = NetworkStatus::Ok as u32 as u64;
        reply.words[1] = 99;
        assert!(network_wifi_join_parse_reply(&reply).is_err());
    }

    #[test]
    fn wifi_scan_parse_reply_decodes_entry_fields() {
        let mut reply = RawMessage::empty(NetworkTag::WifiScanReply as u32);
        reply.word_count = 3 + WIFI_SCAN_ENTRY_WORDS as u32;
        reply.words[0] = NetworkStatus::Ok as u32 as u64;
        reply.words[1] = 1;
        reply.words[2] = 1;
        let bssid48 = 0xaabbccddeeffu64;
        reply.words[3] = (6u64 << 56) | (0xd8u64 << 48) | bssid48;
        reply.words[4] = (4u64 << 56) | (1u64 << 48) | u64::from_le_bytes(*b"home\0\0\0\0");
        reply.words[5] = u64::from_le_bytes(*b"work\0\0\0\0");
        reply.words[6] = 0;
        reply.words[7] = 0;
        reply.words[8] = 0;
        let mut entries = [NetworkWifiScanEntry {
            bssid: [0; 6],
            channel: 0,
            rssi: 0,
            ssid_len: 0,
            ssid: [0; 32],
            security: WifiSecurity::Unknown,
        }; 2];
        let total = network_wifi_scan_parse_reply(&reply, &mut entries).expect("parses");
        assert_eq!(total, 1);
        let entry = &entries[0];
        assert_eq!(entry.bssid, [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
        assert_eq!(entry.channel, 6);
        assert_eq!(entry.rssi, -40);
        assert_eq!(entry.ssid_len, 4);
        assert_eq!(&entry.ssid[..4], b"home");
        assert_eq!(entry.security, WifiSecurity::Wpa2);

        // Unsupported status (no backend) surfaces the honest error.
        let mut absent = RawMessage::empty(NetworkTag::WifiScanReply as u32);
        absent.word_count = 3;
        absent.words[0] = NetworkStatus::Unsupported as u32 as u64;
        absent.words[1] = 0;
        absent.words[2] = 0;
        assert_eq!(
            network_wifi_scan_parse_reply(&absent, &mut entries).expect_err("unsupported"),
            Error::Unsupported
        );
    }

    #[test]
    fn wifi_saved_list_parse_reply_decodes_ssid_and_priority() {
        let mut reply = RawMessage::empty(NetworkTag::WifiSavedListReply as u32);
        reply.word_count = (3 + WIFI_SAVED_ENTRY_WORDS) as u32;
        reply.words[0] = NetworkStatus::Ok as u32 as u64;
        reply.words[1] = 1;
        reply.words[2] = 1;
        reply.words[3] = (4u64 << 56) | (3u64 << 48) | u64::from_le_bytes(*b"home\0\0\0\0");
        reply.words[4] = 0;
        let mut saved = [NetworkWifiSavedNetwork {
            ssid_len: 0,
            ssid: [0; 32],
            priority: 0,
        }; 2];
        let total = network_wifi_saved_list_parse_reply(&reply, &mut saved).expect("parses");
        assert_eq!(total, 1);
        assert_eq!(saved[0].ssid_len, 4);
        assert_eq!(&saved[0].ssid[..4], b"home");
        assert_eq!(saved[0].priority, 3);
    }

    #[test]
    fn wifi_saved_add_remove_request_shapes_roundtrip() {
        let request =
            network_wifi_saved_add_request("home", "passphrase1", 3).expect("request builds");
        assert_eq!(request.tag, NetworkTag::WifiSavedAddRequest as u32);
        assert_eq!(request.word_count, 6);
        assert_eq!(request.words[0], 4);
        assert_eq!(request.words[1], 11);
        assert_eq!(request.words[2], 3);
        // Open-network saves are rejected up front (codec cannot store them).
        assert!(network_wifi_saved_add_request("open", "", 0).is_err());
        assert!(network_wifi_saved_add_request("", "passphrase1", 0).is_err());

        let remove = network_wifi_saved_remove_request("home").expect("request builds");
        assert_eq!(remove.tag, NetworkTag::WifiSavedRemoveRequest as u32);
        assert_eq!(remove.word_count, 2);
        assert_eq!(remove.words[0], 4);
        assert!(network_wifi_saved_remove_request("").is_err());
    }

    #[test]
    fn wifi_status_parse_reply_validates_echo_shape() {
        let mut reply = RawMessage::empty(NetworkTag::WifiStatusReply as u32);
        reply.word_count = 8;
        reply.words[0] = NetworkStatus::Ok as u32 as u64;
        reply.words[1] = WifiLinkState::Connected as u64;
        reply.words[2] = 1;
        reply.words[3] = 4;
        reply.words[4] = u64::from_le_bytes(*b"home\0\0\0\0");
        let status = network_wifi_status_parse_reply(&reply).expect("parses");
        assert_eq!(status.link_state, WifiLinkState::Connected);
        assert!(status.backend_present);
        assert_eq!(&status.ssid[..4], b"home");

        // No backend: honest Unsupported error.
        reply.words[0] = NetworkStatus::Unsupported as u32 as u64;
        assert_eq!(
            network_wifi_status_parse_reply(&reply).expect_err("unsupported"),
            Error::Unsupported
        );

        // Unknown link-state word rejected.
        reply.words[0] = NetworkStatus::Ok as u32 as u64;
        reply.words[1] = 42;
        assert!(network_wifi_status_parse_reply(&reply).is_err());
    }

    #[test]
    fn firewall_rule_word_packs_qualifier_bits() {
        // Legacy shape: no interface qualifier -> bits [48..64) stay zero.
        let legacy = NetworkFirewallRule {
            deny: true,
            proto: NetworkFirewallProto::Tcp,
            direction: NetworkFirewallDirection::Inbound,
            port: 80,
            interface: None,
            enabled: true,
        };
        let word = pack_firewall_rule_word(&legacy);
        assert_eq!(word & 0xff, 1, "deny");
        assert_eq!((word >> 8) & 0xff, NetworkFirewallProto::Tcp as u64);
        assert_eq!(
            (word >> 16) & 0xff,
            NetworkFirewallDirection::Inbound as u64
        );
        assert_eq!((word >> 24) & 1, 1, "enabled");
        assert_eq!((word >> 32) & 0xffff, 80);
        assert_eq!(word >> 48, 0, "unqualified");

        // Pinned rule: eth1 (index 1) encodes as qualifier value 2.
        let pinned = NetworkFirewallRule {
            deny: false,
            proto: NetworkFirewallProto::Udp,
            direction: NetworkFirewallDirection::Outbound,
            port: 0,
            interface: Some(1),
            enabled: false,
        };
        let word = pack_firewall_rule_word(&pinned);
        assert_eq!(word & 0xff, 0, "allow");
        assert_eq!(word >> 48, 2, "interface index 1 -> qualifier 2");
        assert_eq!((word >> 24) & 1, 0, "disabled flag rides the rule word");
    }

    #[test]
    fn firewall_replace_request_carries_qualified_rules() {
        let rules = [
            NetworkFirewallRule {
                deny: true,
                proto: NetworkFirewallProto::Udp,
                direction: NetworkFirewallDirection::Outbound,
                port: 53,
                interface: Some(0),
                enabled: true,
            },
            NetworkFirewallRule {
                deny: false,
                proto: NetworkFirewallProto::Any,
                direction: NetworkFirewallDirection::Inbound,
                port: 443,
                interface: None,
                enabled: true,
            },
        ];
        let request = firewall_replace_request(&rules).expect("request builds");
        assert_eq!(request.tag, NetworkTag::FirewallRulesSetRequest as u32);
        assert_eq!(request.word_count, 6);
        assert_eq!(request.words[0], 0, "op 0 = replace whole table");
        assert_eq!(request.words[1], 2);
        assert_eq!(request.words[2] >> 48, 1, "qualified to index 0");
        assert_eq!(request.words[3], 1, "enable flag");
        assert_eq!(request.words[4] >> 48, 0, "unqualified rule word");
        assert_eq!(request.words[5], 1);

        // Over-budget table rejected before any words are written.
        let too_many = [rules[0]]; // reuse one rule shape for the cap check
        let mut oversized = [too_many[0]; 8];
        oversized[7].interface = None;
        assert!(firewall_replace_request(&oversized).is_err());
    }

    #[test]
    fn firewall_summary_parse_reply_ignores_trailing_rule_words() {
        // A reply carrying a qualified table still parses into the global
        // summary; old readers only touch words 0..3.
        let mut reply = RawMessage::empty(NetworkTag::FirewallRulesReply as u32);
        reply.word_count = 8;
        reply.words[0] = NetworkStatus::Ok as u32 as u64;
        reply.words[1] = 2;
        reply.words[2] = 0;
        reply.words[3] = (3 << 32) | 1;
        reply.words[4] = 1 | (2 << 8) | (1 << 16) | (1 << 24) | (53 << 32) | (1 << 48);
        reply.words[5] = 7;
        reply.words[6] = 0;
        reply.words[7] = 4;
        let summary = network_firewall_summary_parse_reply(&reply).expect("parses");
        assert_eq!(summary.rule_count, 2);
        assert!(!summary.default_inbound_allow);
        assert_eq!(summary.inbound_denied_total, 3);
        assert_eq!(summary.outbound_denied_total, 1);
    }
}
