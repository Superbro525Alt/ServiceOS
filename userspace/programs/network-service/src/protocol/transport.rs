use smoltcp::{
    iface::{Interface, SocketHandle, SocketSet},
    phy::Device,
    socket::{dhcpv4, icmp},
    wire::{
        Icmpv4Packet, Icmpv4Repr, Icmpv6Packet, Icmpv6Repr, IpAddress, IpCidr, Ipv4Address,
        Ipv4Cidr, Ipv6Address, Ipv6Cidr,
    },
};

use rt::{LogEvent, LogSeverity, NetworkConfigMode, NetworkConfigState};
use serviceos_userspace_runtime as rt;

use crate::{
    consts::PING_IDENTIFIER,
    device::KernelPacketDevice,
    types::{InterfaceRuntimeState, NetworkConfig},
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
                    dns_server: configured
                        .dns_servers
                        .first()
                        .copied()
                        .unwrap_or(config.dns_server),
                };
                *dhcp_started_at = rt::monotonic_now().unwrap_or(*dhcp_started_at);
                apply_interface_runtime(iface, *runtime_state);
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
                let _ = emit_log(
                    log_handle,
                    LogSeverity::Warn,
                    LogEvent::NetworkLeaseChanged,
                    0,
                    0,
                );
            }
        }
    }

    if runtime_state.state == NetworkConfigState::Pending
        && rt::monotonic_now()?.saturating_sub(*dhcp_started_at)
            >= config.dhcp_acquire_timeout_ticks
    {
        *runtime_state = InterfaceRuntimeState::static_config(*config);
        apply_interface_runtime(iface, *runtime_state);
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

        let payload = [
            0x53,
            0x4f,
            (*next_sequence >> 8) as u8,
            *next_sequence as u8,
        ];
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
                    let elapsed_ms = ticks_to_millis(rt::monotonic_now()?).saturating_sub(start_ms);
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
    let mac = match iface.hardware_addr() {
        smoltcp::wire::HardwareAddress::Ethernet(address) => address.0,
    };
    let link_local = crate::util::eui64_link_local(mac);
    iface.update_ip_addrs(|addrs| {
        addrs.clear();
        if runtime_state.address != Ipv4Address::UNSPECIFIED && runtime_state.prefix_len != 0 {
            let _ = addrs.push(IpCidr::Ipv4(Ipv4Cidr::new(
                runtime_state.address,
                runtime_state.prefix_len,
            )));
        }
        // Guest-internal loopback: lets UDP/TCP connect to 127.0.0.1 through
        // the device loopback path (see device.rs).
        let _ = addrs.push(IpCidr::Ipv4(Ipv4Cidr::new(
            crate::consts::LOOPBACK_ADDRESS,
            8,
        )));
        // IPv6 v0 slice: exactly one link-local address from the MAC
        // (modified EUI-64). Link-scoped only; no SLAAC, DHCPv6, DAD, or
        // default route (see consts.rs honest-scope note).
        let _ = addrs.push(IpCidr::Ipv6(Ipv6Cidr::new(link_local, 64)));
    });
    crate::device::set_local_ipv4(runtime_state.address);
    crate::device::set_local_ipv6(link_local);
    let _ = iface.routes_mut().remove_default_ipv4_route();
    if runtime_state.gateway != Ipv4Address::UNSPECIFIED {
        let _ = iface
            .routes_mut()
            .add_default_ipv4_route(runtime_state.gateway);
    }
}

/// ICMPv6 echo (ping6) over the shared ICMP socket, mirroring perform_ping.
/// Link-local scope only: the target must already be on-link (no routing,
/// no address resolution beyond the NS/NA exchange smoltcp drives itself).
pub(crate) fn perform_ping6(
    iface: &mut Interface,
    device: &mut KernelPacketDevice,
    sockets: &mut SocketSet<'_>,
    icmp_handle: SocketHandle,
    target: Ipv6Address,
    timeout_ticks: u64,
    next_sequence: &mut u16,
) -> rt::Result<Option<u64>> {
    let start_ticks = rt::monotonic_now()?;
    let start_ms = ticks_to_millis(start_ticks);
    let checksum = device.capabilities().checksum;
    let source = iface.get_source_address_ipv6(&target);

    {
        let socket = sockets.get_mut::<icmp::Socket>(icmp_handle);
        if !socket.is_open() {
            let _ = socket.bind(icmp::Endpoint::Ident(PING_IDENTIFIER));
        }
        if !socket.can_send() {
            return Ok(None);
        }

        let payload = [
            0x53,
            0x4f,
            (*next_sequence >> 8) as u8,
            *next_sequence as u8,
        ];
        let icmp_repr = Icmpv6Repr::EchoRequest {
            ident: PING_IDENTIFIER,
            seq_no: *next_sequence,
            data: &payload,
        };
        let packet = socket
            .send(icmp_repr.buffer_len(), IpAddress::Ipv6(target))
            .map_err(|_| rt::Error::Busy)?;
        // The socket re-emits with the real source address at dispatch time
        // (checksums ignored on that parse), but emitting a fully valid
        // packet keeps the buffer wire-shaped on its own.
        icmp_repr.emit(
            &source,
            &target,
            &mut Icmpv6Packet::new_unchecked(packet),
            &checksum,
        );
    }

    let sequence = *next_sequence;
    *next_sequence = next_sequence.wrapping_add(1);

    loop {
        let _ = iface.poll(now_instant(), device, sockets);
        let socket = sockets.get_mut::<icmp::Socket>(icmp_handle);
        if socket.can_recv() {
            let (payload, remote_addr) = socket.recv().map_err(|_| rt::Error::Busy)?;
            let packet =
                Icmpv6Packet::new_checked(&payload).map_err(|_| rt::Error::InvalidArgument)?;
            let IpAddress::Ipv6(remote) = remote_addr else {
                return Err(rt::Error::InvalidArgument);
            };
            let reply = Icmpv6Repr::parse(&remote, &source, &packet, &checksum)
                .map_err(|_| rt::Error::InvalidArgument)?;
            if let Icmpv6Repr::EchoReply {
                ident,
                seq_no,
                data: _,
            } = reply
            {
                if ident == PING_IDENTIFIER && seq_no == sequence {
                    let elapsed_ms = ticks_to_millis(rt::monotonic_now()?).saturating_sub(start_ms);
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
