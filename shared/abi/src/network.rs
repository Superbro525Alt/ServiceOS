pub const PACKET_INTERFACE_FLAG_NONBLOCK: u32 = 1 << 0;

/// Wire layout of the shared RX packet ring handed to a packet-interface
/// consumer through PacketInterfaceRingSetup. The kernel owns the descriptor
/// head counter; the consumer owns tail. Both sides access the same
/// memory-object-backed pages, so frames are consumed in place with no
/// per-frame IPC copy.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PacketRingLayout {
    pub magic: u32,
    pub version: u32,
    pub slot_count: u32,
    /// Payload bytes per slot (frame data capacity, excluding the length
    /// field).
    pub slot_data_bytes: u32,
    /// Stride between consecutive slots in the shared image; each slot owns
    /// one whole page so a frame never straddles a page boundary.
    pub slot_stride_bytes: u32,
    pub total_bytes: u32,
}

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
    /// Open a listening socket. words[0] = NetworkSocketKind (TcpStream),
    /// words[1] = pack_listen_params(local_port, backlog). Reply: status +
    /// listener control handle; AcceptRequest on that handle yields
    /// established stream handles speaking the standard stream protocol.
    SocketListenRequest = 0x80c,
    SocketListenReply = 0x80d,
    // --- Additive public-channel families promoted from network-service
    // reserved tags (0x80e..=0x821). Wire values are historical and frozen by
    // the `network_tag_promoted_wire_values` test below; append new families
    // only after DiscoveryPeersReply. Note: 0x820/0x821 are unique within THIS
    // channel's tag space; NetworkSocketTag reuses those numerics on the
    // separate per-socket control channels, matching the per-channel namespace
    // convention every tag family in this ABI follows.
    /// Ordered first-match firewall table. One SET op per message via
    /// words[0]: 0 = replace all rules (words[1] count, words[2..] records),
    /// 1 = set default inbound policy (words[1] != 0 allows), 2 = clear
    /// rules. Replies carry the table + hit/deny counters.
    FirewallRulesSetRequest = 0x80e,
    FirewallRulesReply = 0x80f,
    /// Query the full firewall table + counters. Replies
    /// FirewallRulesReply.
    FirewallRulesGetRequest = 0x810,
    /// Extended resolver query: words carry a DNS rdata type (A/AAAA/TXT)
    /// plus name; reply appends a typed detail code to the standard
    /// ResolveReply shape.
    ResolveExRequest = 0x812,
    ResolveExReply = 0x813,
    HostnameGetRequest = 0x814,
    HostnameGetReply = 0x815,
    /// Session-scoped runtime hostname set (default `serviceos`).
    HostnameSetRequest = 0x816,
    HostnameSetReply = 0x817,
    /// Continuous-ping diagnostics: N sequential ICMP probes with per-packet
    /// RTTs folded into min/max/avg/jitter/permil-loss stats.
    DiagPingStatsRequest = 0x818,
    DiagPingStatsReply = 0x819,
    /// ARP-snooped neighbor table dump observed off the RX path.
    NeighborDumpRequest = 0x81a,
    NeighborDumpReply = 0x81b,
    /// Self port-scan: TCP listeners, client UDP sockets, internal service
    /// ports.
    ListenPortsRequest = 0x81c,
    ListenPortsReply = 0x81d,
    /// Local service discovery registry over a service-local UDP beacon
    /// (port 41453, service-local wire format).
    DiscoveryRegisterRequest = 0x81e,
    DiscoveryRegisterReply = 0x81f,
    /// Peer query returning hosts announced within a caller-supplied window.
    DiscoveryPeersRequest = 0x820,
    DiscoveryPeersReply = 0x821,
    // --- Wireless (Wi-Fi) control family. Additive at the end; the
    // service-local 0x822/0x823 zero-copy-stats pair stays out of the ABI
    // until its own promotion. Every request carries a reply handle in
    // handles[0]; every reply leads with words[0] = NetworkStatus. With no
    // WirelessBackend device registered (the only configuration in-tree
    // today) every operation replies `Unsupported` — never fake success.
    /// Trigger a scan. Reply WifiScanReply: words[1] = total networks found,
    /// words[2] = entries in this message, then 5 words per entry
    /// (see the scan-entry packing note below; max 2 entries per reply).
    WifiScanRequest = 0x824,
    WifiScanReply = 0x825,
    /// Join a network: words[0] = ssid length, words[1] = psk length
    /// (0 = open network), words[2..] = inline ssid bytes followed by
    /// inline psk bytes. Reply WifiJoinReply: words[1] = WifiLinkState.
    WifiJoinRequest = 0x826,
    WifiJoinReply = 0x827,
    /// Drop the current wireless link. Reply WifiLeaveReply:
    /// words[1] = WifiLinkState.
    WifiLeaveRequest = 0x828,
    WifiLeaveReply = 0x829,
    /// List saved networks. Reply WifiSavedListReply: words[1] = total saved
    /// count, words[2] = entries in this message, then 5 words per entry
    /// (see the saved-entry packing note below; max 2 per message). PSK
    /// octets never leave the service.
    WifiSavedListRequest = 0x82a,
    WifiSavedListReply = 0x82b,
    /// Remember a network: words[0] = ssid length, words[1] = psk length,
    /// words[2] = priority, words[3..] = inline ssid then psk bytes.
    /// Reply WifiSavedAddReply: status only (Ok / InvalidTarget /
    /// CapacityExceeded).
    WifiSavedAddRequest = 0x82c,
    WifiSavedAddReply = 0x82d,
    /// Forget a network: words[0] = ssid length, words[1..] = inline ssid.
    /// Reply WifiSavedRemoveReply: status only (Ok / NotFound /
    /// InvalidTarget).
    WifiSavedRemoveRequest = 0x82e,
    WifiSavedRemoveReply = 0x82f,
    /// Wireless status echo. Reply WifiStatusReply: words[1] = WifiLinkState,
    /// words[2] = flags (bit 0 = wireless backend registered), words[3] =
    /// current ssid length, words[4..] = inline ssid bytes (0 length when
    /// down). The status word is `Unsupported` while no backend exists —
    /// the state echo is still honest service-side truth.
    WifiStatusRequest = 0x830,
    WifiStatusReply = 0x831,
}

/// Service-layer security classification for a wireless network. Values
/// mirror the pure-layer `Security` classification (RSNE presence + AKM).
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiSecurity {
    Open = 0,
    Wpa2 = 1,
    Wpa3 = 2,
    Unknown = 3,
}

/// Service-visible wireless link phases. Values mirror the pure-layer
/// `LinkState` machine the network-service drives.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiLinkState {
    Down = 0,
    Scanning = 1,
    Authenticating = 2,
    Associating = 3,
    Connected = 4,
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
    /// Connectionless datagram socket. Open with SocketOpenRequest (empty
    /// target, words[1] port = local port, 0 = auto-ephemeral), then drive it
    /// with SendTo/ReceiveFrom/Bind on the returned control handle.
    UdpDatagram = 2,
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

/// Per-socket control-channel tags. All requests carry a reply handle in
/// handles[0]. Semantics are nonblocking-with-status, matching the TCP stream
/// contract: a call that cannot make progress replies `Busy` and clients loop
/// with `yield_current` plus their own timeout, exactly like stream
/// ReceiveRequest. There is no service-side blocking wait.
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
    /// UDP only: (re)bind the datagram socket. words[0] = local port
    /// (0 = invalid). Reply: status.
    BindRequest = 0x828,
    BindReply = 0x829,
    /// UDP only: send one datagram. words[0] = payload length, words[1] =
    /// destination endpoint via pack_ipv4_endpoint, words[2..] = inline
    /// payload. Reply: words[0] = status, words[1] = bytes written.
    SendToRequest = 0x82a,
    SendToReply = 0x82b,
    /// UDP only: receive one datagram. words[0] = max payload length. Reply:
    /// words[0] = status (Busy when no datagram is queued), words[1] =
    /// payload length, words[2] = source endpoint via pack_ipv4_endpoint,
    /// words[3..] = inline payload.
    ReceiveFromRequest = 0x82c,
    ReceiveFromReply = 0x82d,
    /// Listener only: pop one pending inbound connection. Reply: words[0] =
    /// status (Busy when the accept queue is empty), words[1] = remote IPv4
    /// (big-endian u32), words[2] = remote port, handles[0] = established
    /// stream control handle speaking the exact same Status/Send/Receive/
    /// Close protocol as outbound streams.
    AcceptRequest = 0x82e,
    AcceptReply = 0x82f,
}

/// Pack the SocketListenRequest words[1] parameter: local port + backlog.
pub const fn pack_listen_params(local_port: u16, backlog: u32) -> u64 {
    ((local_port as u64) << 32) | backlog as u64
}

/// Unpack the SocketListenRequest words[1] parameter.
pub const fn unpack_listen_params(packed: u64) -> (u16, u32) {
    ((packed >> 32) as u16, packed as u32)
}

/// Pack a UDP SendToRequest destination / ReceiveFromReply source endpoint.
pub const fn pack_ipv4_endpoint(address_be: u32, port: u16) -> u64 {
    ((address_be as u64) << 16) | port as u64
}

/// Unpack a packed IPv4 endpoint word.
pub const fn unpack_ipv4_endpoint(packed: u64) -> (u32, u16) {
    ((packed >> 16) as u32, packed as u16)
}

pub const TCP_FLAG_FIN: u8 = 1 << 0;
pub const TCP_FLAG_SYN: u8 = 1 << 1;
pub const TCP_FLAG_RST: u8 = 1 << 2;
pub const TCP_FLAG_PSH: u8 = 1 << 3;
pub const TCP_FLAG_ACK: u8 = 1 << 4;

/// Reference TCP segment header (RFC 793) wire codec. The runtime data plane
/// delegates to the TCP/IP stack; this codec pins down the wire format the
/// contract speaks so host-side tests can verify parsing/serialization and
/// state-transition semantics without hardware.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TcpSegmentHeader {
    pub src_port: u16,
    pub dst_port: u16,
    pub sequence: u32,
    pub acknowledgment: u32,
    pub flags: u8,
    pub window: u16,
}

impl TcpSegmentHeader {
    pub const WIRE_LEN: usize = 20;

    pub fn emit(&self, out: &mut [u8]) {
        let bytes = out.len().min(Self::WIRE_LEN);
        let (head, rest) = out[..bytes].split_at_mut(bytes.min(16));
        head[0..2].copy_from_slice(&self.src_port.to_be_bytes());
        head[2..4].copy_from_slice(&self.dst_port.to_be_bytes());
        head[4..8].copy_from_slice(&self.sequence.to_be_bytes());
        head[8..12].copy_from_slice(&self.acknowledgment.to_be_bytes());
        head[12] = 5 << 4;
        head[13] = self.flags;
        head[14..16].copy_from_slice(&self.window.to_be_bytes());
        if let Some(rest) = rest.get_mut(..4) {
            rest.copy_from_slice(&[0, 0, 0, 0]);
        }
    }

    pub fn parse(segment: &[u8]) -> Option<Self> {
        if segment.len() < Self::WIRE_LEN {
            return None;
        }
        let data_offset = segment[12] >> 4;
        if data_offset < 5 {
            return None;
        }
        Some(Self {
            src_port: u16::from_be_bytes([segment[0], segment[1]]),
            dst_port: u16::from_be_bytes([segment[2], segment[3]]),
            sequence: u32::from_be_bytes([segment[4], segment[5], segment[6], segment[7]]),
            acknowledgment: u32::from_be_bytes([segment[8], segment[9], segment[10], segment[11]]),
            flags: segment[13],
            window: u16::from_be_bytes([segment[14], segment[15]]),
        })
    }

    pub fn has(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }
}

/// Reference UDP datagram header (RFC 768) wire codec. `checksum` uses the
/// Internet checksum over the pseudo-header plus the datagram; see
/// [`udp_checksum`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UdpDatagramHeader {
    pub src_port: u16,
    pub dst_port: u16,
    pub length: u16,
    pub checksum: u16,
}

impl UdpDatagramHeader {
    pub const WIRE_LEN: usize = 8;

    pub fn emit(&self, out: &mut [u8]) {
        if out.len() < Self::WIRE_LEN {
            return;
        }
        out[0..2].copy_from_slice(&self.src_port.to_be_bytes());
        out[2..4].copy_from_slice(&self.dst_port.to_be_bytes());
        out[4..6].copy_from_slice(&self.length.to_be_bytes());
        out[6..8].copy_from_slice(&self.checksum.to_be_bytes());
    }

    pub fn parse(datagram: &[u8]) -> Option<Self> {
        if datagram.len() < Self::WIRE_LEN {
            return None;
        }
        Some(Self {
            src_port: u16::from_be_bytes([datagram[0], datagram[1]]),
            dst_port: u16::from_be_bytes([datagram[2], datagram[3]]),
            length: u16::from_be_bytes([datagram[4], datagram[5]]),
            checksum: u16::from_be_bytes([datagram[6], datagram[7]]),
        })
    }
}

/// RFC 1071 Internet checksum over `data` (odd lengths zero-padded).
pub fn internet_checksum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    let mut chunks = data.chunks_exact(2);
    for chunk in &mut chunks {
        sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
    }
    if let [last] = chunks.remainder() {
        sum += (*last as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// UDP checksum (RFC 768) over the IPv4 pseudo-header plus the datagram
/// (header + payload). Returns 0xffff for an all-zero computed checksum so a
/// transmitted datagram never carries the "no checksum" value.
pub fn udp_checksum(src_ip: [u8; 4], dst_ip: [u8; 4], datagram: &[u8]) -> u16 {
    let mut pseudo = [0u8; 12];
    pseudo[0..4].copy_from_slice(&src_ip);
    pseudo[4..8].copy_from_slice(&dst_ip);
    pseudo[9] = 17;
    pseudo[10..12].copy_from_slice(&(datagram.len() as u16).to_be_bytes());

    let mut sum = 0u32;
    for part in [&pseudo[..], datagram] {
        let mut chunks = part.chunks_exact(2);
        for chunk in &mut chunks {
            sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
        }
        if let [last] = chunks.remainder() {
            sum += (*last as u32) << 8;
        }
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    let checksum = !(sum as u16);
    if checksum == 0 { 0xffff } else { checksum }
}

/// Reference TCP state transitions for the contract's coarse socket states,
/// driven by segment events. Mirrors the RFC 793 diagram subset the network
/// service exposes: SYN progress is `Connecting`, data phase is
/// `Established`, teardown (FIN/RST) is `Closing`/`Closed`. The stack remains
/// authoritative at runtime; this table defines what clients may observe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TcpSegmentEvent {
    Syn,
    SynAck,
    Ack,
    Fin,
    Rst,
}

pub fn tcp_reference_transition(
    state: NetworkSocketState,
    event: TcpSegmentEvent,
) -> NetworkSocketState {
    use NetworkSocketState::{Closed, Closing, Connecting, Established, Failed};
    use TcpSegmentEvent::{Ack, Fin, Rst, Syn, SynAck};
    match (state, event) {
        // Handshake progress: a SYN opens a connection, SYNACK/ACK completes it.
        (Closed, Syn) | (Connecting, Syn) => Connecting,
        (Connecting, SynAck | Ack) => Established,
        // Teardown during handshake aborts straight to Closed.
        (Connecting, Fin | Rst) => Closed,
        // Data phase holds until teardown begins.
        (Established, Ack | Syn | SynAck) => Established,
        (Established, Fin) => Closing,
        (Established, Rst) => Closed,
        // Graceful close waits for the peer's FIN/ACKs; only RST aborts.
        (Closing, Rst) => Closed,
        (Closing, Syn | SynAck | Ack | Fin) => Closing,
        // Terminal states are absorbing.
        (Failed, _) => Failed,
        (Closed, SynAck | Ack | Fin | Rst) => Closed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internet_checksum_rfc1071_vector() {
        // RFC 1071 worked example: sum 0x0001 + 0xf203 + 0xf4f5 + 0xf6f7
        // = 0xddf2, checksum = 0x220d.
        let data = [0x00, 0x01, 0xf2, 0x03, 0xf4, 0xf5, 0xf6, 0xf7];
        assert_eq!(internet_checksum(&data), 0x220d);
    }

    #[test]
    fn internet_checksum_odd_length_and_verification() {
        // Verification property: data followed by its checksum folds to zero.
        let data = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x88];
        let checksum = internet_checksum(&data);
        let mut framed = [0u8; 10];
        framed[..8].copy_from_slice(&data);
        framed[8] = (checksum >> 8) as u8;
        framed[9] = (checksum & 0xff) as u8;
        assert_eq!(internet_checksum(&framed), 0);

        let odd = [0xab, 0xcd, 0xef];
        let odd_checksum = internet_checksum(&odd);
        // Odd trailing byte is treated as high-order byte of a padded word.
        let mut framed = [0xab, 0xcd, 0xef, 0x00, 0x00, 0x00];
        framed[4] = (odd_checksum >> 8) as u8;
        framed[5] = (odd_checksum & 0xff) as u8;
        assert_eq!(internet_checksum(&framed), 0);
    }

    #[test]
    fn udp_checksum_pseudo_header_and_zero_rule() {
        let datagram = [0x00, 0x35, 0x00, 0x35, 0x00, 0x09, 0x00, 0x00, 0x68, 0x69];
        let checksum = udp_checksum([10, 0, 2, 15], [10, 0, 2, 2], &datagram);
        // Verification property: pseudo-header + datagram + checksum folds to 0xffff.
        let mut framed = [0u8; 12 + 10];
        framed[0..4].copy_from_slice(&[10, 0, 2, 15]);
        framed[4..8].copy_from_slice(&[10, 0, 2, 2]);
        framed[9] = 17;
        framed[10..12].copy_from_slice(&(datagram.len() as u16).to_be_bytes());
        framed[12..22].copy_from_slice(&datagram);
        framed[18] = (checksum >> 8) as u8;
        framed[19] = (checksum & 0xff) as u8;
        let mut sum = 0u32;
        for chunk in framed.chunks_exact(2) {
            sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
        }
        while sum >> 16 != 0 {
            sum = (sum & 0xffff) + (sum >> 16);
        }
        assert_eq!(sum, 0xffff);
        // Flipping any payload byte must change the checksum.
        let mut flipped = datagram;
        flipped[9] ^= 0x01;
        assert_ne!(
            udp_checksum([10, 0, 2, 15], [10, 0, 2, 2], &flipped),
            checksum
        );
        // Different destination must change the checksum (pseudo-header bound).
        assert_ne!(
            udp_checksum([10, 0, 2, 15], [10, 0, 2, 3], &datagram),
            checksum
        );
        // A computation that lands on zero is transmitted as 0xffff, never 0.
        // proto 0x11 + length 2 + data 0xffec folds to 0xffff -> ~0 = 0.
        assert_eq!(
            udp_checksum([0, 0, 0, 0], [0, 0, 0, 0], &[0xff, 0xec]),
            0xffff
        );
    }

    #[test]
    fn udp_header_roundtrip_and_rejects_short() {
        let datagram = [
            0x12, 0x34, 0x00, 0x56, 0x00, 0x0c, 0xab, 0xcd, 0x01, 0x02, 0x03, 0x04,
        ];
        let header = UdpDatagramHeader::parse(&datagram).expect("parses");
        assert_eq!(
            header,
            UdpDatagramHeader {
                src_port: 0x1234,
                dst_port: 0x0056,
                length: 12,
                checksum: 0xabcd,
            }
        );
        let mut wire = [0u8; UdpDatagramHeader::WIRE_LEN];
        header.emit(&mut wire);
        assert_eq!(&wire[..], &datagram[..8]);
        assert!(UdpDatagramHeader::parse(&datagram[..7]).is_none());
    }

    #[test]
    fn tcp_segment_roundtrip_and_flags() {
        let header = TcpSegmentHeader {
            src_port: 49152,
            dst_port: 80,
            sequence: 0x11223344,
            acknowledgment: 0x55667788,
            flags: TCP_FLAG_SYN | TCP_FLAG_ACK,
            window: 1024,
        };
        let mut wire = [0u8; 24];
        header.emit(&mut wire);
        let parsed = TcpSegmentHeader::parse(&wire).expect("parses");
        assert_eq!(parsed, header);
        assert!(parsed.has(TCP_FLAG_SYN) && parsed.has(TCP_FLAG_ACK));
        assert!(!parsed.has(TCP_FLAG_FIN) && !parsed.has(TCP_FLAG_RST));
        // Data offset nibble says 5 (20-byte header, no options).
        assert_eq!(wire[12] >> 4, 5);
        assert!(TcpSegmentHeader::parse(&wire[..19]).is_none());
    }

    #[test]
    fn tcp_state_transitions_syn_ack_fin_rst() {
        use NetworkSocketState as S;
        use TcpSegmentEvent::*;
        // Handshake: SYN opens, SYNACK/ACK completes.
        assert_eq!(tcp_reference_transition(S::Closed, Syn), S::Connecting);
        assert_eq!(tcp_reference_transition(S::Connecting, Syn), S::Connecting);
        assert_eq!(
            tcp_reference_transition(S::Connecting, SynAck),
            S::Established
        );
        assert_eq!(tcp_reference_transition(S::Connecting, Ack), S::Established);
        // Data phase holds until teardown.
        assert_eq!(
            tcp_reference_transition(S::Established, Ack),
            S::Established
        );
        // FIN starts graceful close; RST aborts straight to Closed.
        assert_eq!(tcp_reference_transition(S::Established, Fin), S::Closing);
        assert_eq!(tcp_reference_transition(S::Closing, Ack), S::Closing);
        assert_eq!(tcp_reference_transition(S::Closing, Fin), S::Closing);
        assert_eq!(tcp_reference_transition(S::Established, Rst), S::Closed);
        assert_eq!(tcp_reference_transition(S::Closing, Rst), S::Closed);
        // RST/FIN during handshake aborts.
        assert_eq!(tcp_reference_transition(S::Connecting, Rst), S::Closed);
        assert_eq!(tcp_reference_transition(S::Connecting, Fin), S::Closed);
        // Failed stays failed; Closed ignores stray segments.
        assert_eq!(tcp_reference_transition(S::Failed, Fin), S::Failed);
        assert_eq!(tcp_reference_transition(S::Closed, Ack), S::Closed);
    }

    #[test]
    fn ipc_param_codecs_roundtrip() {
        let packed = pack_listen_params(8080, 7);
        assert_eq!(unpack_listen_params(packed), (8080, 7));
        assert_eq!(unpack_listen_params(pack_listen_params(0, 0)), (0, 0));
        assert_eq!(
            unpack_listen_params(pack_listen_params(u16::MAX, u32::MAX)),
            (u16::MAX, u32::MAX)
        );

        let endpoint = pack_ipv4_endpoint(0x0a00_020f, 5353);
        assert_eq!(unpack_ipv4_endpoint(endpoint), (0x0a00_020f, 5353));
        assert_eq!(unpack_ipv4_endpoint(pack_ipv4_endpoint(0, 0)), (0, 0));
    }

    #[test]
    fn network_tag_promoted_wire_values() {
        use NetworkTag as T;
        // Historical reserved-tag numbers from network-service; promotion into
        // the shared ABI must never renumber them (wire contract).
        assert_eq!(T::FirewallRulesSetRequest as u32, 0x80e);
        assert_eq!(T::FirewallRulesReply as u32, 0x80f);
        assert_eq!(T::FirewallRulesGetRequest as u32, 0x810);
        assert_eq!(T::ResolveExRequest as u32, 0x812);
        assert_eq!(T::ResolveExReply as u32, 0x813);
        assert_eq!(T::HostnameGetRequest as u32, 0x814);
        assert_eq!(T::HostnameGetReply as u32, 0x815);
        assert_eq!(T::HostnameSetRequest as u32, 0x816);
        assert_eq!(T::HostnameSetReply as u32, 0x817);
        assert_eq!(T::DiagPingStatsRequest as u32, 0x818);
        assert_eq!(T::DiagPingStatsReply as u32, 0x819);
        assert_eq!(T::NeighborDumpRequest as u32, 0x81a);
        assert_eq!(T::NeighborDumpReply as u32, 0x81b);
        assert_eq!(T::ListenPortsRequest as u32, 0x81c);
        assert_eq!(T::ListenPortsReply as u32, 0x81d);
        assert_eq!(T::DiscoveryRegisterRequest as u32, 0x81e);
        assert_eq!(T::DiscoveryRegisterReply as u32, 0x81f);
        assert_eq!(T::DiscoveryPeersRequest as u32, 0x820);
        assert_eq!(T::DiscoveryPeersReply as u32, 0x821);
        // Wireless family appended after the promoted block.
        assert_eq!(T::WifiScanRequest as u32, 0x824);
        assert_eq!(T::WifiScanReply as u32, 0x825);
        assert_eq!(T::WifiJoinRequest as u32, 0x826);
        assert_eq!(T::WifiJoinReply as u32, 0x827);
        assert_eq!(T::WifiLeaveRequest as u32, 0x828);
        assert_eq!(T::WifiLeaveReply as u32, 0x829);
        assert_eq!(T::WifiSavedListRequest as u32, 0x82a);
        assert_eq!(T::WifiSavedListReply as u32, 0x82b);
        assert_eq!(T::WifiSavedAddRequest as u32, 0x82c);
        assert_eq!(T::WifiSavedAddReply as u32, 0x82d);
        assert_eq!(T::WifiSavedRemoveRequest as u32, 0x82e);
        assert_eq!(T::WifiSavedRemoveReply as u32, 0x82f);
        assert_eq!(T::WifiStatusRequest as u32, 0x830);
        assert_eq!(T::WifiStatusReply as u32, 0x831);
        // Pre-existing public-channel tags keep their numbers too.
        assert_eq!(T::SocketListenReply as u32, 0x80d);
        // Cross-namespace sanity: per-socket control channels are a distinct
        // tag space, so 0x820 legally appears in both enums.
        assert_eq!(NetworkSocketTag::StatusRequest as u32, 0x820);
    }
}
