use rt::{
    NetworkSocketInfo, NetworkSocketKind, NetworkSocketState, NetworkWifiSavedNetwork,
    NetworkWifiScanEntry, ServiceId, WifiLinkState, WifiSecurity,
};
use serviceos_userspace_runtime as rt;

use crate::util::{
    ShellOutput, format_ipv4, format_mac, link_state_name, network_config_mode_name,
    network_config_state_name, network_socket_state_name, shell_output_write, write_output_linef,
};

const MAX_NETWORK_SOCKETS: usize = 2;
const HTTP_CONNECT_TIMEOUT_TICKS: u64 = 600;
const HTTP_READ_TIMEOUT_TICKS: u64 = 400;
const HTTP_CHUNK_BYTES: usize = (rt::IPC_MAX_WORDS - 2) * 8;

pub(crate) fn cmd_net<'a, I>(
    bootstrap: rt::Handle,
    output: ShellOutput,
    mut parts: I,
) -> rt::Result<()>
where
    I: Iterator<Item = &'a str>,
{
    match parts.next() {
        Some("ifaces") => cmd_net_ifaces(bootstrap, output),
        Some("route") => cmd_net_route(bootstrap, output),
        Some("sockets") => cmd_net_sockets(bootstrap, output),
        Some("resolve") => match parts.next() {
            Some(target) => cmd_net_resolve(bootstrap, output, target),
            None => write_output_linef(output, format_args!("usage: net resolve <name>")),
        },
        Some("ping") => match parts.next() {
            Some(target) => cmd_net_ping(bootstrap, output, target),
            None => write_output_linef(output, format_args!("usage: net ping <name|ip>")),
        },
        Some("http") => match parts.next() {
            Some(host) => cmd_net_http(bootstrap, output, host, parts.next().unwrap_or("/")),
            None => write_output_linef(output, format_args!("usage: net http <host> [path]")),
        },
        _ => write_output_linef(
            output,
            format_args!("usage: net <ifaces|route|sockets|resolve|ping|http> ..."),
        ),
    }
}

fn cmd_net_ifaces(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let network_handle = rt::lookup_service(bootstrap, ServiceId::Network)?;
    let count = rt::network_interface_count(network_handle)?;
    if count == 0 {
        let _ = rt::handle_close(network_handle);
        return write_output_linef(output, format_args!("no interfaces"));
    }

    for index in 0..count {
        if let Some(info) = rt::network_interface_status(network_handle, index)? {
            write_output_linef(
                output,
                format_args!(
                    "net{} link={} cfg={}/{} mtu={} mac={}",
                    info.index,
                    link_state_name(info.link_state),
                    network_config_mode_name(info.config_mode),
                    network_config_state_name(info.config_state),
                    info.mtu,
                    format_mac(info.mac),
                ),
            )?;
            write_output_linef(
                output,
                format_args!(
                    "  addr={}/{} gw={} dns={} rx={} tx={} drop={}",
                    format_ipv4(info.address),
                    info.prefix_len,
                    format_ipv4(info.gateway),
                    format_ipv4(info.dns_server),
                    info.rx_packets,
                    info.tx_packets,
                    info.dropped_packets,
                ),
            )?;
        }
    }

    let _ = rt::handle_close(network_handle);
    Ok(())
}

fn cmd_net_route(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let network_handle = rt::lookup_service(bootstrap, ServiceId::Network)?;
    let info = rt::network_interface_status(network_handle, 0)?;
    let _ = rt::handle_close(network_handle);
    match info {
        Some(info) => write_output_linef(
            output,
            format_args!(
                "default via {} dev net{} cfg={}/{}",
                format_ipv4(info.gateway),
                info.index,
                network_config_mode_name(info.config_mode),
                network_config_state_name(info.config_state),
            ),
        ),
        None => write_output_linef(output, format_args!("no default route")),
    }
}

fn cmd_net_sockets(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let network_handle = rt::lookup_service(bootstrap, ServiceId::Network)?;
    let mut sockets = [NetworkSocketInfo {
        slot: 0,
        kind: NetworkSocketKind::TcpStream,
        state: NetworkSocketState::Closed,
        remote_address: 0,
        remote_port: 0,
        local_port: 0,
        rx_bytes: 0,
        tx_bytes: 0,
    }; MAX_NETWORK_SOCKETS];
    let count = rt::network_socket_list(network_handle, &mut sockets)?;
    let _ = rt::handle_close(network_handle);

    if count == 0 {
        return write_output_linef(output, format_args!("no active sockets"));
    }

    for socket in sockets.iter().take(count) {
        write_output_linef(
            output,
            format_args!(
                "sock{} tcp state={} remote={}:{} local={} rx={} tx={}",
                socket.slot,
                network_socket_state_name(socket.state),
                format_ipv4(socket.remote_address),
                socket.remote_port,
                socket.local_port,
                socket.rx_bytes,
                socket.tx_bytes,
            ),
        )?;
    }
    Ok(())
}

fn cmd_net_resolve(bootstrap: rt::Handle, output: ShellOutput, target: &str) -> rt::Result<()> {
    let network_handle = rt::lookup_service(bootstrap, ServiceId::Network)?;
    let mut addresses = [0u32; 4];
    let count = match rt::network_resolve(network_handle, target, &mut addresses) {
        Ok(count) => count,
        Err(rt::Error::NotFound) => {
            let _ = rt::handle_close(network_handle);
            return write_output_linef(output, format_args!("no address for {}", target));
        }
        Err(error) => {
            let _ = rt::handle_close(network_handle);
            return Err(error);
        }
    };
    let _ = rt::handle_close(network_handle);
    if count == 0 {
        return write_output_linef(output, format_args!("no result"));
    }
    for address in addresses.iter().copied().take(count) {
        write_output_linef(
            output,
            format_args!("{} -> {}", target, format_ipv4(address)),
        )?;
    }
    Ok(())
}

fn cmd_net_ping(bootstrap: rt::Handle, output: ShellOutput, target: &str) -> rt::Result<()> {
    let network_handle = rt::lookup_service(bootstrap, ServiceId::Network)?;
    let result = rt::network_ping(network_handle, target);
    let _ = rt::handle_close(network_handle);
    match result {
        Ok((resolved, elapsed_ms)) => write_output_linef(
            output,
            format_args!(
                "ping {} ({}) ok {}ms",
                target,
                format_ipv4(resolved),
                elapsed_ms,
            ),
        ),
        Err(rt::Error::QueueEmpty) => {
            write_output_linef(output, format_args!("ping {} timed out", target))
        }
        Err(rt::Error::NotFound) => {
            write_output_linef(output, format_args!("ping target not found: {}", target))
        }
        Err(error) => Err(error),
    }
}

fn cmd_net_http(
    bootstrap: rt::Handle,
    output: ShellOutput,
    host: &str,
    path: &str,
) -> rt::Result<()> {
    let network_handle = rt::lookup_service(bootstrap, ServiceId::Network)?;
    let socket_handle =
        match rt::network_socket_open(network_handle, NetworkSocketKind::TcpStream, host, 80) {
            Ok(handle) => handle,
            Err(error) => {
                let _ = rt::handle_close(network_handle);
                return write_output_linef(
                    output,
                    format_args!("http connect failed: {}", crate::util::error_name(error)),
                );
            }
        };
    let _ = rt::handle_close(network_handle);

    let result = http_fetch(output, socket_handle, host, path);
    let _ = rt::network_socket_close(socket_handle);
    let _ = rt::handle_close(socket_handle);
    result
}

fn http_fetch(
    output: ShellOutput,
    socket_handle: rt::Handle,
    host: &str,
    path: &str,
) -> rt::Result<()> {
    wait_for_socket_established(socket_handle, HTTP_CONNECT_TIMEOUT_TICKS)?;

    let request_path = if path.is_empty() { "/" } else { path };
    let mut request = rt::FixedLogBuffer::<256>::new();
    use core::fmt::Write;
    let _ = write!(
        &mut request,
        "GET {} HTTP/1.0\r\nHost: {}\r\nUser-Agent: serviceos-shell\r\nConnection: close\r\n\r\n",
        request_path, host,
    );
    let bytes = request.as_bytes();
    let _ = rt::network_socket_send(socket_handle, bytes)?;

    let mut buffer = [0u8; HTTP_CHUNK_BYTES];
    let mut last_progress = rt::monotonic_now()?;
    let mut received_any = false;
    loop {
        match rt::network_socket_receive(socket_handle, &mut buffer) {
            Ok(count) if count > 0 => {
                received_any = true;
                last_progress = rt::monotonic_now()?;
                let text = core::str::from_utf8(&buffer[..count])
                    .map_err(|_| rt::Error::InvalidArgument)?;
                shell_output_write(output, text)?;
            }
            Ok(_) => {}
            Err(rt::Error::Busy) => {}
            Err(rt::Error::NotFound) => {
                let status = rt::network_socket_status(socket_handle)?;
                if matches!(
                    status.state,
                    NetworkSocketState::Closed | NetworkSocketState::Failed
                ) {
                    break;
                }
            }
            Err(error) => return Err(error),
        }

        let status = rt::network_socket_status(socket_handle)?;
        if matches!(
            status.state,
            NetworkSocketState::Closed | NetworkSocketState::Failed
        ) {
            break;
        }
        if rt::monotonic_now()?.saturating_sub(last_progress) >= HTTP_READ_TIMEOUT_TICKS {
            if received_any {
                break;
            }
            return write_output_linef(output, format_args!("\r\nhttp read timed out"));
        }
        rt::yield_current()?;
    }

    if !matches!(
        rt::network_socket_status(socket_handle)?.state,
        NetworkSocketState::Closed | NetworkSocketState::Failed
    ) {
        write_output_linef(output, format_args!("\r\nhttp done"))?;
    }
    Ok(())
}


// --- wifi command family (wireless control plane) ---

pub(crate) fn cmd_wifi<'a, I>(
    bootstrap: rt::Handle,
    output: ShellOutput,
    mut parts: I,
) -> rt::Result<()>
where
    I: Iterator<Item = &'a str>,
{
    match parts.next() {
        Some("scan") => cmd_wifi_scan(bootstrap, output),
        Some("join") => match parts.next() {
            Some(ssid) => {
                let psk = parts.next().filter(|psk| !psk.is_empty());
                cmd_wifi_join(bootstrap, output, ssid, psk)
            }
            None => write_output_linef(output, format_args!("usage: wifi join <ssid> [psk]")),
        },
        Some("leave") => cmd_wifi_leave(bootstrap, output),
        Some("saved") => match parts.next() {
            None | Some("list") => cmd_wifi_saved_list(bootstrap, output),
            Some("add") => match (parts.next(), parts.next()) {
                (Some(ssid), Some(psk)) if !psk.is_empty() => {
                    cmd_wifi_saved_add(bootstrap, output, ssid, psk)
                }
                _ => write_output_linef(output, format_args!("usage: wifi saved add <ssid> <psk>")),
            },
            Some("remove") => match parts.next() {
                Some(ssid) => cmd_wifi_saved_remove(bootstrap, output, ssid),
                None => write_output_linef(output, format_args!("usage: wifi saved remove <ssid>")),
            },
            _ => write_output_linef(
                output,
                format_args!("usage: wifi saved [list|add <ssid> <psk>|remove <ssid>]"),
            ),
        },
        Some("status") => cmd_wifi_status(bootstrap, output),
        _ => write_output_linef(
            output,
            format_args!("usage: wifi <scan|join <ssid> [psk]|leave|saved|status> ..."),
        ),
    }
}

pub(crate) fn wifi_security_name(security: WifiSecurity) -> &'static str {
    match security {
        WifiSecurity::Open => "open",
        WifiSecurity::Wpa2 => "wpa2",
        WifiSecurity::Wpa3 => "wpa3",
        WifiSecurity::Unknown => "unknown",
    }
}

pub(crate) fn wifi_link_state_name(state: WifiLinkState) -> &'static str {
    match state {
        WifiLinkState::Down => "down",
        WifiLinkState::Scanning => "scanning",
        WifiLinkState::Authenticating => "authenticating",
        WifiLinkState::Associating => "associating",
        WifiLinkState::Connected => "connected",
    }
}

/// Renders an SSID octet slice as UTF-8 text, or a placeholder when the
/// beacon carried none (wildcard/hidden) or the octets are not UTF-8.
fn wifi_ssid_text(ssid: &[u8]) -> &str {
    if ssid.is_empty() {
        "<hidden>"
    } else {
        core::str::from_utf8(ssid).unwrap_or("<binary>")
    }
}

fn cmd_wifi_scan(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let network_handle = rt::lookup_service(bootstrap, ServiceId::Network)?;
    let mut entries = [NetworkWifiScanEntry {
        bssid: [0; 6],
        channel: 0,
        rssi: 0,
        ssid_len: 0,
        ssid: [0; 32],
        security: WifiSecurity::Unknown,
    }; rt::NETWORK_WIFI_SCAN_REPLY_ENTRIES_MAX];
    let result = rt::network_wifi_scan(network_handle, &mut entries);
    let _ = rt::handle_close(network_handle);
    match result {
        Ok(total) if total == 0 => {
            write_output_linef(output, format_args!("wifi scan: no networks"))
        }
        Ok(total) => {
            for entry in entries.iter().take(total) {
                let ssid = wifi_ssid_text(&entry.ssid[..entry.ssid_len]);
                write_output_linef(
                    output,
                    format_args!(
                        "wifi scan: ch{} rssi{} {} {} {:02x?}",
                        entry.channel,
                        entry.rssi,
                        wifi_security_name(entry.security),
                        ssid,
                        entry.bssid,
                    ),
                )?;
            }
            Ok(())
        }
        Err(rt::Error::Unsupported) => {
            write_output_linef(output, format_args!("wifi scan: no wireless backend"))
        }
        Err(error) => Err(error),
    }
}

fn cmd_wifi_join(
    bootstrap: rt::Handle,
    output: ShellOutput,
    ssid: &str,
    psk: Option<&str>,
) -> rt::Result<()> {
    let network_handle = rt::lookup_service(bootstrap, ServiceId::Network)?;
    let result = rt::network_wifi_join(network_handle, ssid, psk);
    let _ = rt::handle_close(network_handle);
    match result {
        Ok(state) => write_output_linef(
            output,
            format_args!("wifi join {}: link={}", ssid, wifi_link_state_name(state)),
        ),
        Err(rt::Error::Unsupported) => {
            write_output_linef(output, format_args!("wifi join: no wireless backend"))
        }
        Err(rt::Error::InvalidArgument) => write_output_linef(
            output,
            format_args!("wifi join: invalid ssid or psk (psk 8..=64 chars, or none for open)"),
        ),
        Err(error) => Err(error),
    }
}

fn cmd_wifi_leave(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let network_handle = rt::lookup_service(bootstrap, ServiceId::Network)?;
    let result = rt::network_wifi_leave(network_handle);
    let _ = rt::handle_close(network_handle);
    match result {
        Ok(state) => write_output_linef(
            output,
            format_args!("wifi leave: link={}", wifi_link_state_name(state)),
        ),
        Err(rt::Error::Unsupported) => {
            write_output_linef(output, format_args!("wifi leave: no wireless backend"))
        }
        Err(error) => Err(error),
    }
}

fn cmd_wifi_saved_list(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let network_handle = rt::lookup_service(bootstrap, ServiceId::Network)?;
    let mut saved = [NetworkWifiSavedNetwork {
        ssid_len: 0,
        ssid: [0; 32],
        priority: 0,
    }; rt::NETWORK_WIFI_SAVED_REPLY_ENTRIES_MAX];
    let result = rt::network_wifi_saved_list(network_handle, &mut saved);
    let _ = rt::handle_close(network_handle);
    match result {
        Ok(total) if total == 0 => write_output_linef(output, format_args!("wifi saved: none")),
        Ok(total) => {
            for record in saved.iter().take(total) {
                let ssid =
                    core::str::from_utf8(&record.ssid[..record.ssid_len]).unwrap_or("<invalid>");
                write_output_linef(
                    output,
                    format_args!("wifi saved: {} priority={}", ssid, record.priority),
                )?;
            }
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn cmd_wifi_saved_add(
    bootstrap: rt::Handle,
    output: ShellOutput,
    ssid: &str,
    psk: &str,
) -> rt::Result<()> {
    let network_handle = rt::lookup_service(bootstrap, ServiceId::Network)?;
    let result = rt::network_wifi_saved_add(network_handle, ssid, psk, 0);
    let _ = rt::handle_close(network_handle);
    match result {
        Ok(()) => write_output_linef(output, format_args!("wifi saved: added {}", ssid)),
        Err(rt::Error::CapacityExceeded) => {
            write_output_linef(output, format_args!("wifi saved: store full"))
        }
        Err(rt::Error::InvalidArgument) => write_output_linef(
            output,
            format_args!("wifi saved: invalid ssid or psk (psk required, 8..=64 chars)"),
        ),
        Err(error) => Err(error),
    }
}

fn cmd_wifi_saved_remove(
    bootstrap: rt::Handle,
    output: ShellOutput,
    ssid: &str,
) -> rt::Result<()> {
    let network_handle = rt::lookup_service(bootstrap, ServiceId::Network)?;
    let result = rt::network_wifi_saved_remove(network_handle, ssid);
    let _ = rt::handle_close(network_handle);
    match result {
        Ok(()) => write_output_linef(output, format_args!("wifi saved: removed {}", ssid)),
        Err(rt::Error::NotFound) => {
            write_output_linef(output, format_args!("wifi saved: {} not found", ssid))
        }
        Err(error) => Err(error),
    }
}

fn cmd_wifi_status(bootstrap: rt::Handle, output: ShellOutput) -> rt::Result<()> {
    let network_handle = rt::lookup_service(bootstrap, ServiceId::Network)?;
    let result = rt::network_wifi_status(network_handle);
    let _ = rt::handle_close(network_handle);
    match result {
        Ok(status) => {
            let ssid = wifi_ssid_text(&status.ssid[..status.ssid_len]);
            write_output_linef(
                output,
                format_args!(
                    "wifi status: link={} backend={} ssid={}",
                    wifi_link_state_name(status.link_state),
                    if status.backend_present { "yes" } else { "no" },
                    if ssid.is_empty() { "<none>" } else { ssid },
                ),
            )
        }
        Err(rt::Error::Unsupported) => {
            write_output_linef(output, format_args!("wifi status: no wireless backend"))
        }
        Err(error) => Err(error),
    }
}

fn wait_for_socket_established(socket_handle: rt::Handle, timeout_ticks: u64) -> rt::Result<()> {
    let start = rt::monotonic_now()?;
    loop {
        let status = rt::network_socket_status(socket_handle)?;
        match status.state {
            NetworkSocketState::Established => return Ok(()),
            NetworkSocketState::Failed | NetworkSocketState::Closed => {
                return Err(rt::Error::NotFound);
            }
            NetworkSocketState::Connecting | NetworkSocketState::Closing => {}
        }
        if rt::monotonic_now()?.saturating_sub(start) >= timeout_ticks {
            return Err(rt::Error::QueueEmpty);
        }
        rt::yield_current()?;
    }
}

#[cfg(test)]
mod wifi_tests {
    use super::*;

    #[test]
    fn wifi_security_names_cover_every_classification() {
        assert_eq!(wifi_security_name(WifiSecurity::Open), "open");
        assert_eq!(wifi_security_name(WifiSecurity::Wpa2), "wpa2");
        assert_eq!(wifi_security_name(WifiSecurity::Wpa3), "wpa3");
        assert_eq!(wifi_security_name(WifiSecurity::Unknown), "unknown");
    }

    #[test]
    fn wifi_link_state_names_cover_every_phase() {
        assert_eq!(wifi_link_state_name(WifiLinkState::Down), "down");
        assert_eq!(wifi_link_state_name(WifiLinkState::Scanning), "scanning");
        assert_eq!(
            wifi_link_state_name(WifiLinkState::Authenticating),
            "authenticating"
        );
        assert_eq!(
            wifi_link_state_name(WifiLinkState::Associating),
            "associating"
        );
        assert_eq!(wifi_link_state_name(WifiLinkState::Connected), "connected");
    }

    #[test]
    fn wifi_ssid_text_renders_hidden_and_binary_placeholders() {
        assert_eq!(wifi_ssid_text(b"home"), "home");
        assert_eq!(wifi_ssid_text(b""), "<hidden>");
        assert_eq!(wifi_ssid_text(&[0xff, 0xfe]), "<binary>");
    }

    #[test]
    fn wifi_scan_entry_field_layout_matches_runtime_type() {
        // Guards the field names the renderer destructures; the runtime type
        // is the wire contract (bssid/channel/rssi/ssid/security).
        let entry = NetworkWifiScanEntry {
            bssid: [0x10, 0x20, 0x30, 0x40, 0x50, 0x60],
            channel: 6,
            rssi: -40,
            ssid_len: 4,
            ssid: {
                let mut ssid = [0u8; 32];
                ssid[..4].copy_from_slice(b"home");
                ssid
            },
            security: WifiSecurity::Wpa2,
        };
        assert_eq!(entry.ssid_len, 4);
        assert_eq!(entry.channel, 6);
        assert_eq!(entry.rssi, -40);
        assert_eq!(wifi_ssid_text(&entry.ssid[..entry.ssid_len]), "home");
        assert_eq!(wifi_security_name(entry.security), "wpa2");
    }
}
