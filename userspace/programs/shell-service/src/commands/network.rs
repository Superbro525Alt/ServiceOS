use serviceos_userspace_runtime as rt;
use rt::ServiceId;

use crate::util::{
    format_ipv4, format_mac, link_state_name, write_session_linef,
};

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
        Some("resolve") => match parts.next() {
            Some(target) => cmd_net_resolve(bootstrap, session, target),
            None => write_session_linef(session, format_args!("usage: net resolve <name>")),
        },
        Some("ping") => match parts.next() {
            Some(target) => cmd_net_ping(bootstrap, session, target),
            None => write_session_linef(session, format_args!("usage: net ping <name|ip>")),
        },
        _ => write_session_linef(session, format_args!("usage: net <ifaces|route|resolve|ping> ...")),
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
                    "net{} link={} addr={}/{} gw={} mac={} mtu={} rx={} tx={} drop={}",
                    info.index,
                    link_state_name(info.link_state),
                    format_ipv4(info.address),
                    info.prefix_len,
                    format_ipv4(info.gateway),
                    format_mac(info.mac),
                    info.mtu,
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
            format_args!("default via {} dev net{}", format_ipv4(info.gateway), info.index),
        ),
        None => write_session_linef(session, format_args!("no default route")),
    }
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
