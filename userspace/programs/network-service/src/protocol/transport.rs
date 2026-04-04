use smoltcp::{
    iface::{Interface, SocketHandle, SocketSet},
    phy::Device,
    socket::{dhcpv4, dns, icmp},
    wire::{
        DnsQueryType, Icmpv4Packet, Icmpv4Repr, IpAddress, IpCidr, Ipv4Address, Ipv4Cidr,
    },
};

use serviceos_userspace_runtime as rt;
use rt::{LogEvent, LogSeverity, NetworkConfigMode, NetworkConfigState};

use crate::{
    config::parse_ipv4,
    consts::PING_IDENTIFIER,
    device::KernelPacketDevice,
    types::{HostEntry, InterfaceRuntimeState, NetworkConfig},
    util::{emit_log, ipv4_to_u32, now_instant, ticks_to_millis},
};

pub(crate) fn drive_dynamic_ipv4(
    config: &NetworkConfig,
    log_handle: rt::Handle,
    runtime_state: &mut InterfaceRuntimeState,
    dhcp_started_at: &mut u64,
    iface: &mut Interface,
    sockets: &mut SocketSet<'_>,
    dhcp_handle: SocketHandle,
    dns_handle: SocketHandle,
) -> rt::Result<()> {
    if let Some(event) = sockets.get_mut::<dhcpv4::Socket>(dhcp_handle).poll() {
        match event {
            dhcpv4::Event::Configured(configured) => {
                *runtime_state = InterfaceRuntimeState {
                    mode: NetworkConfigMode::Dynamic,
                    state: NetworkConfigState::Configured,
                    address: configured.address.address(),
                    prefix_len: configured.address.prefix_len(),
                    gateway: configured.router.unwrap_or(Ipv4Address::UNSPECIFIED),
                    dns_server: configured.dns_servers.first().copied().unwrap_or(config.dns_server),
                };
                *dhcp_started_at = rt::monotonic_now().unwrap_or(*dhcp_started_at);
                apply_interface_runtime(iface, *runtime_state);
                update_dns_servers(
                    sockets.get_mut::<dns::Socket>(dns_handle),
                    runtime_state.dns_server,
                );
                let _ = emit_log(
                    log_handle,
                    LogSeverity::Info,
                    LogEvent::NetworkLeaseChanged,
                    ipv4_to_u32(runtime_state.address) as u64,
                    ipv4_to_u32(runtime_state.gateway) as u64,
                );
                let _ = emit_log(
                    log_handle,
                    LogSeverity::Info,
                    LogEvent::NetworkAddressConfigured,
                    ipv4_to_u32(runtime_state.address) as u64,
                    ipv4_to_u32(runtime_state.gateway) as u64,
                );
            }
            dhcpv4::Event::Deconfigured => {
                *runtime_state = InterfaceRuntimeState::pending_dynamic();
                *dhcp_started_at = rt::monotonic_now().unwrap_or(*dhcp_started_at);
                apply_interface_runtime(iface, *runtime_state);
                update_dns_servers(
                    sockets.get_mut::<dns::Socket>(dns_handle),
                    runtime_state.dns_server,
                );
                let _ = emit_log(log_handle, LogSeverity::Warn, LogEvent::NetworkLeaseChanged, 0, 0);
            }
        }
    }

    if runtime_state.state == NetworkConfigState::Pending
        && rt::monotonic_now()?.saturating_sub(*dhcp_started_at) >= config.dhcp_acquire_timeout_ticks
    {
        *runtime_state = InterfaceRuntimeState::static_config(*config);
        apply_interface_runtime(iface, *runtime_state);
        update_dns_servers(
            sockets.get_mut::<dns::Socket>(dns_handle),
            runtime_state.dns_server,
        );
        let _ = emit_log(
            log_handle,
            LogSeverity::Warn,
            LogEvent::NetworkLeaseChanged,
            ipv4_to_u32(runtime_state.address) as u64,
            ipv4_to_u32(runtime_state.gateway) as u64,
        );
        let _ = emit_log(
            log_handle,
            LogSeverity::Warn,
            LogEvent::NetworkAddressConfigured,
            ipv4_to_u32(runtime_state.address) as u64,
            ipv4_to_u32(runtime_state.gateway) as u64,
        );
    }

    Ok(())
}

pub(crate) fn resolve_target(
    target: &str,
    hosts: &[HostEntry],
    timeout_ticks: u64,
    iface: &mut Interface,
    device: &mut KernelPacketDevice,
    sockets: &mut SocketSet<'_>,
    dns_handle: SocketHandle,
) -> rt::Result<Option<Ipv4Address>> {
    if let Some(address) = parse_ipv4(target) {
        return Ok(Some(address));
    }
    if let Some(address) = hosts
        .iter()
        .find(|entry| entry.name_len != 0 && entry.matches(target))
        .map(|entry| entry.address)
    {
        return Ok(Some(address));
    }

    let query = {
        let socket = sockets.get_mut::<dns::Socket>(dns_handle);
        match socket.start_query(iface.context(), target, DnsQueryType::A) {
            Ok(handle) => handle,
            Err(_) => return Ok(None),
        }
    };
    let start_ticks = rt::monotonic_now()?;
    loop {
        let _ = iface.poll(now_instant(), device, sockets);
        match sockets.get_mut::<dns::Socket>(dns_handle).get_query_result(query) {
            Ok(addresses) => {
                for address in addresses {
                    let IpAddress::Ipv4(ipv4) = address;
                    return Ok(Some(ipv4));
                }
                return Ok(None);
            }
            Err(dns::GetQueryResultError::Pending) => {}
            Err(_) => return Ok(None),
        }

        if rt::monotonic_now()?.saturating_sub(start_ticks) >= timeout_ticks {
            sockets.get_mut::<dns::Socket>(dns_handle).cancel_query(query);
            return Ok(None);
        }

        rt::yield_current()?;
    }
}

pub(crate) fn perform_ping(
    iface: &mut Interface,
    device: &mut KernelPacketDevice,
    sockets: &mut SocketSet<'_>,
    icmp_handle: SocketHandle,
    target: Ipv4Address,
    timeout_ticks: u64,
    next_sequence: &mut u16,
) -> rt::Result<Option<u64>> {
    let start_ticks = rt::monotonic_now()?;
    let start_ms = ticks_to_millis(start_ticks);
    let checksum = device.capabilities().checksum;

    {
        let socket = sockets.get_mut::<icmp::Socket>(icmp_handle);
        if !socket.is_open() {
            let _ = socket.bind(icmp::Endpoint::Ident(PING_IDENTIFIER));
        }
        if !socket.can_send() {
            return Ok(None);
        }

        let payload = [0x53, 0x4f, (*next_sequence >> 8) as u8, *next_sequence as u8];
        let icmp_repr = Icmpv4Repr::EchoRequest {
            ident: PING_IDENTIFIER,
            seq_no: *next_sequence,
            data: &payload,
        };
        let packet = socket
            .send(icmp_repr.buffer_len(), IpAddress::Ipv4(target))
            .map_err(|_| rt::Error::Busy)?;
        icmp_repr.emit(&mut Icmpv4Packet::new_unchecked(packet), &checksum);
    }

    let sequence = *next_sequence;
    *next_sequence = next_sequence.wrapping_add(1);

    loop {
        let _ = iface.poll(now_instant(), device, sockets);
        let socket = sockets.get_mut::<icmp::Socket>(icmp_handle);
        if socket.can_recv() {
            let (payload, _) = socket.recv().map_err(|_| rt::Error::Busy)?;
            let packet =
                Icmpv4Packet::new_checked(&payload).map_err(|_| rt::Error::InvalidArgument)?;
            let reply =
                Icmpv4Repr::parse(&packet, &checksum).map_err(|_| rt::Error::InvalidArgument)?;
            if let Icmpv4Repr::EchoReply {
                ident,
                seq_no,
                data: _,
            } = reply
            {
                if ident == PING_IDENTIFIER && seq_no == sequence {
                    let elapsed_ms =
                        ticks_to_millis(rt::monotonic_now()?).saturating_sub(start_ms);
                    return Ok(Some(elapsed_ms));
                }
            }
        }

        if rt::monotonic_now()?.saturating_sub(start_ticks) >= timeout_ticks {
            return Ok(None);
        }

        rt::yield_current()?;
    }
}

pub(crate) fn apply_interface_runtime(iface: &mut Interface, runtime_state: InterfaceRuntimeState) {
    iface.update_ip_addrs(|addrs| {
        addrs.clear();
        if runtime_state.address != Ipv4Address::UNSPECIFIED && runtime_state.prefix_len != 0 {
            let _ = addrs.push(IpCidr::Ipv4(Ipv4Cidr::new(
                runtime_state.address,
                runtime_state.prefix_len,
            )));
        }
    });
    let _ = iface.routes_mut().remove_default_ipv4_route();
    if runtime_state.gateway != Ipv4Address::UNSPECIFIED {
        let _ = iface.routes_mut().add_default_ipv4_route(runtime_state.gateway);
    }
}

pub(crate) fn update_dns_servers(socket: &mut dns::Socket<'_>, server: Ipv4Address) {
    let servers = dns_server_list(server);
    socket.update_servers(&servers);
}

fn dns_server_list(server: Ipv4Address) -> [IpAddress; 1] {
    [IpAddress::Ipv4(server)]
}
