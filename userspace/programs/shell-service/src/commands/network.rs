use serviceos_userspace_runtime as rt;
use rt::{NetworkSocketInfo, NetworkSocketKind, NetworkSocketState, ServiceId};

use crate::util::{
    format_ipv4, format_mac, link_state_name, network_config_mode_name, network_config_state_name,
    network_socket_state_name, write_session_linef, write_session_text,
};

const MAX_NETWORK_SOCKETS: usize = 2;
const HTTP_CONNECT_TIMEOUT_TICKS: u64 = 600;
const HTTP_READ_TIMEOUT_TICKS: u64 = 400;
const HTTP_CHUNK_BYTES: usize = (rt::IPC_MAX_WORDS - 2) * 8;

pub(crate) fn cmd_net<'a, I>(
    bootstrap: rt::Handle,
    session: rt::Handle,
    mut parts: I,
) -> rt::Result<()>
where
    I: Iterator<Item = &'a str>,
{
    match parts.next() {
        Some("ifaces") => cmd_net_ifaces(bootstrap, session),
        Some("route") => cmd_net_route(bootstrap, session),
        Some("sockets") => cmd_net_sockets(bootstrap, session),
        Some("resolve") => match parts.next() {
            Some(target) => cmd_net_resolve(bootstrap, session, target),
            None => write_session_linef(session, format_args!("usage: net resolve <name>")),
        },
        Some("ping") => match parts.next() {
            Some(target) => cmd_net_ping(bootstrap, session, target),
            None => write_session_linef(session, format_args!("usage: net ping <name|ip>")),
        },
        Some("http") => match parts.next() {
            Some(host) => cmd_net_http(bootstrap, session, host, parts.next().unwrap_or("/")),
            None => write_session_linef(session, format_args!("usage: net http <host> [path]")),
        },
        _ => write_session_linef(
            session,
            format_args!("usage: net <ifaces|route|sockets|resolve|ping|http> ..."),
        ),
    }
}

fn cmd_net_ifaces(bootstrap: rt::Handle, session: rt::Handle) -> rt::Result<()> {
    let network_handle = rt::lookup_service(bootstrap, ServiceId::Network)?;
    let count = rt::network_interface_count(network_handle)?;
    if count == 0 {
        let _ = rt::handle_close(network_handle);
        return write_session_linef(session, format_args!("no interfaces"));
    }

    for index in 0..count {
        if let Some(info) = rt::network_interface_status(network_handle, index)? {
            write_session_linef(
                session,
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
            write_session_linef(
                session,
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

fn cmd_net_route(bootstrap: rt::Handle, session: rt::Handle) -> rt::Result<()> {
    let network_handle = rt::lookup_service(bootstrap, ServiceId::Network)?;
    let info = rt::network_interface_status(network_handle, 0)?;
    let _ = rt::handle_close(network_handle);
    match info {
        Some(info) => write_session_linef(
            session,
            format_args!(
                "default via {} dev net{} cfg={}/{}",
                format_ipv4(info.gateway),
                info.index,
                network_config_mode_name(info.config_mode),
                network_config_state_name(info.config_state),
            ),
        ),
        None => write_session_linef(session, format_args!("no default route")),
    }
}

fn cmd_net_sockets(bootstrap: rt::Handle, session: rt::Handle) -> rt::Result<()> {
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
        return write_session_linef(session, format_args!("no active sockets"));
    }

    for socket in sockets.iter().take(count) {
        write_session_linef(
            session,
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

fn cmd_net_resolve(bootstrap: rt::Handle, session: rt::Handle, target: &str) -> rt::Result<()> {
    let network_handle = rt::lookup_service(bootstrap, ServiceId::Network)?;
    let mut addresses = [0u32; 4];
    let count = match rt::network_resolve(network_handle, target, &mut addresses) {
        Ok(count) => count,
        Err(rt::Error::NotFound) => {
            let _ = rt::handle_close(network_handle);
            return write_session_linef(session, format_args!("no address for {}", target));
        }
        Err(error) => {
            let _ = rt::handle_close(network_handle);
            return Err(error);
        }
    };
    let _ = rt::handle_close(network_handle);
    if count == 0 {
        return write_session_linef(session, format_args!("no result"));
    }
    for address in addresses.iter().copied().take(count) {
        write_session_linef(session, format_args!("{} -> {}", target, format_ipv4(address)))?;
    }
    Ok(())
}

fn cmd_net_ping(bootstrap: rt::Handle, session: rt::Handle, target: &str) -> rt::Result<()> {
    let network_handle = rt::lookup_service(bootstrap, ServiceId::Network)?;
    let result = rt::network_ping(network_handle, target);
    let _ = rt::handle_close(network_handle);
    match result {
        Ok((resolved, elapsed_ms)) => write_session_linef(
            session,
            format_args!(
                "ping {} ({}) ok {}ms",
                target,
                format_ipv4(resolved),
                elapsed_ms,
            ),
        ),
        Err(rt::Error::QueueEmpty) => {
            write_session_linef(session, format_args!("ping {} timed out", target))
        }
        Err(rt::Error::NotFound) => {
            write_session_linef(session, format_args!("ping target not found: {}", target))
        }
        Err(error) => Err(error),
    }
}

fn cmd_net_http(
    bootstrap: rt::Handle,
    session: rt::Handle,
    host: &str,
    path: &str,
) -> rt::Result<()> {
    let network_handle = rt::lookup_service(bootstrap, ServiceId::Network)?;
    let socket_handle = match rt::network_socket_open(
        network_handle,
        NetworkSocketKind::TcpStream,
        host,
        80,
    ) {
        Ok(handle) => handle,
        Err(error) => {
            let _ = rt::handle_close(network_handle);
            return write_session_linef(
                session,
                format_args!("http connect failed: {}", crate::util::error_name(error)),
            );
        }
    };
    let _ = rt::handle_close(network_handle);

    let result = http_fetch(session, socket_handle, host, path);
    let _ = rt::network_socket_close(socket_handle);
    let _ = rt::handle_close(socket_handle);
    result
}

fn http_fetch(
    session: rt::Handle,
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
        request_path,
        host,
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
                write_session_text(session, text)?;
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
        if matches!(status.state, NetworkSocketState::Closed | NetworkSocketState::Failed) {
            break;
        }
        if rt::monotonic_now()?.saturating_sub(last_progress) >= HTTP_READ_TIMEOUT_TICKS {
            if received_any {
                break;
            }
            return write_session_linef(session, format_args!("\r\nhttp read timed out"));
        }
        rt::yield_current()?;
    }

    if !matches!(
        rt::network_socket_status(socket_handle)?.state,
        NetworkSocketState::Closed | NetworkSocketState::Failed
    ) {
        write_session_linef(session, format_args!("\r\nhttp done"))?;
    }
    Ok(())
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
