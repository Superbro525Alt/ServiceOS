use core::fmt::Write as _;

use rt::FixedLogBuffer;
use serviceos_userspace_runtime as rt;

use crate::state::{AppState, PING_PROBE_COUNT, PING_TARGET_MAX_BYTES};

pub(crate) fn format_ipv4<const N: usize>(address: u32) -> FixedLogBuffer<N> {
    let mut buffer = FixedLogBuffer::<N>::new();
    let _ = write!(
        &mut buffer,
        "{}.{}.{}.{}",
        (address >> 24) & 0xff,
        (address >> 16) & 0xff,
        (address >> 8) & 0xff,
        address & 0xff,
    );
    buffer
}

pub(crate) fn format_mac<const N: usize>(mac: [u8; 6]) -> FixedLogBuffer<N> {
    let mut buffer = FixedLogBuffer::<N>::new();
    let _ = write!(
        &mut buffer,
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5],
    );
    buffer
}

pub(crate) fn format_loss<const N: usize>(loss_permil: u64) -> FixedLogBuffer<N> {
    let mut buffer = FixedLogBuffer::<N>::new();
    let _ = write!(&mut buffer, "{}.{}%", loss_permil / 10, loss_permil % 10,);
    buffer
}

pub(crate) fn link_state_name(state: rt::PacketInterfaceLinkState) -> &'static str {
    match state {
        rt::PacketInterfaceLinkState::Up => "UP",
        rt::PacketInterfaceLinkState::Down => "DOWN",
    }
}

pub(crate) fn listen_port_kind_name(kind: rt::NetworkListenPortKind) -> &'static str {
    match kind {
        rt::NetworkListenPortKind::TcpListener => "TCP",
        rt::NetworkListenPortKind::UdpClient => "UDP",
        rt::NetworkListenPortKind::UdpInternal => "UDP-I",
        rt::NetworkListenPortKind::Unknown => "?",
    }
}

/// Pick a literal ping target: gateway first, then the DNS server.
pub(crate) fn ping_target_address(
    interface: Option<rt::NetworkInterfaceStatusInfo>,
) -> Option<u32> {
    let interface = interface?;
    if interface.gateway != 0 {
        return Some(interface.gateway);
    }
    if interface.dns_server != 0 {
        return Some(interface.dns_server);
    }
    None
}

/// One diagnostics run against the literal dotted target. Transport or
/// resolve failures land in `ping_failed` instead of panicking; the page
/// renders an honest "unavailable" line afterwards.
pub(crate) fn run_ping(network_handle: rt::Handle, target: u32, state: &mut AppState) {
    let dotted = format_ipv4::<PING_TARGET_MAX_BYTES>(target);
    state.ping_target[..dotted.as_bytes().len()].copy_from_slice(dotted.as_bytes());
    state.ping_target_len = dotted.as_bytes().len();
    match rt::network_diag_ping_stats(network_handle, dotted.as_str(), PING_PROBE_COUNT) {
        Ok(stats) => {
            state.ping_stats = Some(stats);
            state.ping_failed = false;
        }
        Err(_) => {
            state.ping_stats = None;
            state.ping_failed = true;
        }
    }
}

/// Commit the hostname edit buffer via the session-scoped runtime wrapper.
/// Returns false when the name is empty, over-long, or the service rejects
/// it — the page keeps the edit open so the operator can retry.
pub(crate) fn commit_hostname(network_handle: rt::Handle, state: &mut AppState) -> bool {
    let Ok(name) = core::str::from_utf8(&state.hostname_edit[..state.hostname_edit_len]) else {
        return false;
    };
    if name.is_empty() {
        return false;
    }
    matches!(rt::network_hostname_set(network_handle, name), Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv4_text_formats_dotted_quad() {
        assert_eq!(format_ipv4::<16>(0x0a00_020f).as_str(), "10.0.2.15");
        assert_eq!(format_ipv4::<16>(0).as_str(), "0.0.0.0");
    }

    #[test]
    fn mac_text_formats_colon_pairs() {
        assert_eq!(
            format_mac::<18>([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]).as_str(),
            "52:54:00:12:34:56"
        );
    }

    #[test]
    fn loss_text_renders_permil_as_percent() {
        assert_eq!(format_loss::<16>(0).as_str(), "0.0%");
        assert_eq!(format_loss::<16>(125).as_str(), "12.5%");
        assert_eq!(format_loss::<16>(1000).as_str(), "100.0%");
    }

    #[test]
    fn link_and_port_names_are_honest() {
        assert_eq!(link_state_name(rt::PacketInterfaceLinkState::Up), "UP");
        assert_eq!(link_state_name(rt::PacketInterfaceLinkState::Down), "DOWN");
        assert_eq!(
            listen_port_kind_name(rt::NetworkListenPortKind::TcpListener),
            "TCP"
        );
        assert_eq!(
            listen_port_kind_name(rt::NetworkListenPortKind::UdpInternal),
            "UDP-I"
        );
        assert_eq!(
            listen_port_kind_name(rt::NetworkListenPortKind::Unknown),
            "?"
        );
    }

    #[test]
    fn ping_target_prefers_gateway_then_dns_then_none() {
        let mut interface = rt::NetworkInterfaceStatusInfo {
            index: 0,
            backend: rt::PacketInterfaceBackend::VirtioPci,
            link_state: rt::PacketInterfaceLinkState::Up,
            mtu: 1500,
            config_mode: rt::NetworkConfigMode::Dynamic,
            config_state: rt::NetworkConfigState::Configured,
            address: 0x0a00_020f,
            prefix_len: 24,
            gateway: 0x0a00_0202,
            dns_server: 0x0a00_0203,
            mac: [0; 6],
            rx_packets: 0,
            tx_packets: 0,
            dropped_packets: 0,
            resolver_hits: 0,
            resolver_misses: 0,
        };
        assert_eq!(ping_target_address(Some(interface)), Some(0x0a00_0202));

        interface.gateway = 0;
        assert_eq!(ping_target_address(Some(interface)), Some(0x0a00_0203));

        interface.dns_server = 0;
        assert_eq!(ping_target_address(Some(interface)), None);
        assert_eq!(ping_target_address(None), None);
    }

    #[test]
    fn run_ping_on_invalid_handle_degrades_to_failed_flag() {
        let mut state = AppState {
            width: 320,
            height: 300,
            focused: true,
            page: crate::state::SettingsPage::Network,
            editing_note: false,
            editing_hostname: false,
            selected_policy_index: 0,
            note: [0; crate::state::NOTE_MAX_BYTES],
            note_len: 0,
            hostname_edit: [0; crate::state::HOSTNAME_EDIT_MAX_BYTES],
            hostname_edit_len: 0,
            ping_stats: None,
            ping_failed: false,
            ping_target: [0; PING_TARGET_MAX_BYTES],
            ping_target_len: 0,
        };
        run_ping(rt::INVALID_HANDLE, 0x0a00_0202, &mut state);
        assert!(state.ping_failed);
        assert!(state.ping_stats.is_none());
        assert_eq!(&state.ping_target[..state.ping_target_len], b"10.0.2.2");
    }
}
