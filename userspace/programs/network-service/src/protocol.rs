use smoltcp::{
    iface::{Interface, SocketHandle, SocketSet},
    phy::Device,
    socket::{dhcpv4, dns, icmp, tcp},
    wire::{
        DnsQueryType, Icmpv4Packet, Icmpv4Repr, IpAddress, IpCidr, Ipv4Address, Ipv4Cidr,
    },
};

use serviceos_userspace_runtime as rt;
use rt::{
    LogEvent, LogSeverity, NetworkConfigMode, NetworkConfigState, NetworkSocketKind,
    NetworkSocketState, NetworkSocketTag, NetworkStatus, NetworkTag, RawMessage,
};

use crate::{
    config::parse_ipv4,
    consts::{
        EPHEMERAL_PORT_BASE, MAX_HOSTNAME_BYTES, MAX_SOCKET_INLINE_BYTES, MAX_TCP_SOCKETS,
        PING_IDENTIFIER,
    },
    device::KernelPacketDevice,
    types::{HostEntry, InterfaceRuntimeState, NetworkConfig, TcpTransportSlot},
    util::{
        decode_inline_bytes, decode_inline_text, emit_log, ipv4_to_u32, now_instant,
        pack_inline_bytes, ticks_to_millis,
    },
};

#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_public_request(
    request: &RawMessage,
    packet_handle: rt::Handle,
    log_handle: rt::Handle,
    config: NetworkConfig,
    runtime_state: InterfaceRuntimeState,
    hosts: &[HostEntry],
    iface: &mut Interface,
    device: &mut KernelPacketDevice,
    sockets: &mut SocketSet<'_>,
    dns_handle: SocketHandle,
    icmp_handle: SocketHandle,
    next_sequence: &mut u16,
    transports: &mut [TcpTransportSlot; MAX_TCP_SOCKETS],
    tcp_handles: [SocketHandle; MAX_TCP_SOCKETS],
    next_local_port: &mut u16,
) -> rt::Result<()> {
    match request.tag {
        x if x == NetworkTag::InterfaceListRequest as u32 => {
            if request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(NetworkTag::InterfaceListReply as u32);
            reply.word_count = 2;
            reply.words[0] = NetworkStatus::Ok as u32 as u64;
            reply.words[1] = 1;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == NetworkTag::InterfaceStatusRequest as u32 => {
            if request.word_count < 1 || request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let index = request.words[0] as usize;
            let mut reply = RawMessage::empty(NetworkTag::InterfaceStatusReply as u32);
            reply.word_count = 15;
            if index != 0 {
                reply.words[0] = NetworkStatus::NotFound as u32 as u64;
            } else {
                let info = rt::packet_interface_info(packet_handle)?;
                reply.words[0] = NetworkStatus::Ok as u32 as u64;
                reply.words[1] = 0;
                reply.words[2] = info.backend as u64;
                reply.words[3] = info.link_state as u64;
                reply.words[4] = info.mtu as u64;
                reply.words[5] = runtime_state.mode as u32 as u64;
                reply.words[6] = runtime_state.state as u32 as u64;
                reply.words[7] = ipv4_to_u32(runtime_state.address) as u64;
                reply.words[8] = runtime_state.prefix_len as u64;
                reply.words[9] = ipv4_to_u32(runtime_state.gateway) as u64;
                reply.words[10] = ipv4_to_u32(runtime_state.dns_server) as u64;
                reply.words[11] = crate::util::pack_mac(info.mac);
                reply.words[12] = info.rx_packets;
                reply.words[13] = info.tx_packets;
                reply.words[14] = info.dropped_packets;
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == NetworkTag::ResolveRequest as u32 => {
            if request.word_count < 1 || request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut text = [0u8; MAX_HOSTNAME_BYTES];
            let target = decode_inline_text(
                &request.words[1..request.word_count as usize],
                request.words[0] as usize,
                &mut text,
            )?;
            let mut reply = RawMessage::empty(NetworkTag::ResolveReply as u32);
            reply.word_count = 2;
            if runtime_state.state == NetworkConfigState::Pending && parse_ipv4(target).is_none() {
                reply.words[0] = NetworkStatus::Busy as u32 as u64;
                reply.words[1] = 0;
            } else {
                match resolve_target(
                    target,
                    hosts,
                    config.dns_query_timeout_ticks,
                    iface,
                    device,
                    sockets,
                    dns_handle,
                )? {
                    Some(address) => {
                        reply.words[0] = NetworkStatus::Ok as u32 as u64;
                        reply.words[1] = 1;
                        reply.words[2] = ipv4_to_u32(address) as u64;
                        reply.word_count = 3;
                        let _ = emit_log(
                            log_handle,
                            LogSeverity::Debug,
                            LogEvent::NetworkResolveCompleted,
                            ipv4_to_u32(address) as u64,
                            1,
                        );
                    }
                    None => {
                        reply.words[0] = NetworkStatus::NotFound as u32 as u64;
                        reply.words[1] = 0;
                    }
                }
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == NetworkTag::PingRequest as u32 => {
            if request.word_count < 1 || request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut text = [0u8; MAX_HOSTNAME_BYTES];
            let target = decode_inline_text(
                &request.words[1..request.word_count as usize],
                request.words[0] as usize,
                &mut text,
            )?;
            let mut reply = RawMessage::empty(NetworkTag::PingReply as u32);
            reply.word_count = 3;

            if runtime_state.address == Ipv4Address::UNSPECIFIED {
                reply.words[0] = NetworkStatus::Busy as u32 as u64;
                reply.words[1] = 0;
                reply.words[2] = 0;
            } else {
                match resolve_target(
                    target,
                    hosts,
                    config.dns_query_timeout_ticks,
                    iface,
                    device,
                    sockets,
                    dns_handle,
                )? {
                    Some(address) => match perform_ping(
                        iface,
                        device,
                        sockets,
                        icmp_handle,
                        address,
                        config.probe_timeout_ticks,
                        next_sequence,
                    )? {
                        Some(elapsed_ms) => {
                            reply.words[0] = NetworkStatus::Ok as u32 as u64;
                            reply.words[1] = ipv4_to_u32(address) as u64;
                            reply.words[2] = elapsed_ms;
                            let _ = emit_log(
                                log_handle,
                                LogSeverity::Info,
                                LogEvent::NetworkProbeCompleted,
                                ipv4_to_u32(address) as u64,
                                elapsed_ms,
                            );
                        }
                        None => {
                            reply.words[0] = NetworkStatus::Timeout as u32 as u64;
                            reply.words[1] = ipv4_to_u32(address) as u64;
                            reply.words[2] = 0;
                        }
                    },
                    None => {
                        reply.words[0] = NetworkStatus::NotFound as u32 as u64;
                        reply.words[1] = 0;
                        reply.words[2] = 0;
                    }
                }
            }

            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == NetworkTag::SocketOpenRequest as u32 => {
            if request.word_count < 2 || request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let kind = match request.words[0] as u32 {
                x if x == NetworkSocketKind::TcpStream as u32 => NetworkSocketKind::TcpStream,
                _ => NetworkSocketKind::TcpStream,
            };
            let packed = request.words[1];
            let target_len = (packed >> 16) as usize;
            let remote_port = packed as u16;
            let mut text = [0u8; MAX_HOSTNAME_BYTES];
            let target = decode_inline_text(
                &request.words[2..request.word_count as usize],
                target_len,
                &mut text,
            )?;
            let mut reply = RawMessage::empty(NetworkTag::SocketOpenReply as u32);
            reply.word_count = 1;

            if runtime_state.address == Ipv4Address::UNSPECIFIED || remote_port == 0 {
                reply.words[0] = NetworkStatus::Busy as u32 as u64;
            } else if kind != NetworkSocketKind::TcpStream {
                reply.words[0] = NetworkStatus::Unsupported as u32 as u64;
            } else if let Some(remote_address) = resolve_target(
                target,
                hosts,
                config.dns_query_timeout_ticks,
                iface,
                device,
                sockets,
                dns_handle,
            )? {
                if let Some(slot_index) = allocate_transport_slot(transports) {
                    let session = rt::channel_create()?;
                    let local_port = allocate_ephemeral_port(next_local_port);
                    let socket_handle = tcp_handles[slot_index];
                    let connected = {
                        let socket = sockets.get_mut::<tcp::Socket>(socket_handle);
                        if socket.is_open() {
                            socket.abort();
                        }
                        socket
                            .connect(
                                iface.context(),
                                (IpAddress::Ipv4(remote_address), remote_port),
                                local_port,
                            )
                            .is_ok()
                    };
                    if connected {
                        transports[slot_index] = TcpTransportSlot {
                            active: true,
                            control_handle: session.first,
                            socket_handle: Some(socket_handle),
                            state: NetworkSocketState::Connecting,
                            remote_address,
                            remote_port,
                            local_port,
                            rx_bytes: 0,
                            tx_bytes: 0,
                            opened_at_ticks: rt::monotonic_now()?,
                            last_activity_ticks: rt::monotonic_now()?,
                        };
                        reply.words[0] = NetworkStatus::Ok as u32 as u64;
                        reply.handle_count = 1;
                        reply.handles[0] = session.second;
                        reply.handle_rights[0] = rt::rights::SEND | rt::rights::RECEIVE;
                        let _ = emit_log(
                            log_handle,
                            LogSeverity::Info,
                            LogEvent::NetworkSocketOpened,
                            ipv4_to_u32(remote_address) as u64,
                            remote_port as u64,
                        );
                        let _ = rt::channel_send(reply_handle, &reply);
                        let _ = rt::handle_close(session.second);
                        let _ = rt::handle_close(reply_handle);
                        return Ok(());
                    }
                    let _ = rt::handle_close(session.first);
                    let _ = rt::handle_close(session.second);
                    reply.words[0] = NetworkStatus::Busy as u32 as u64;
                } else {
                    reply.words[0] = NetworkStatus::CapacityExceeded as u32 as u64;
                }
            } else {
                reply.words[0] = NetworkStatus::NotFound as u32 as u64;
            }

            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == NetworkTag::SocketListRequest as u32 => {
            if request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(NetworkTag::SocketListReply as u32);
            reply.words[0] = NetworkStatus::Ok as u32 as u64;
            let mut count = 0usize;
            for slot in transports.iter().filter(|slot| slot.active) {
                if 2 + (count + 1) * 7 > rt::IPC_MAX_WORDS {
                    break;
                }
                let base = 2 + count * 7;
                reply.words[base] = count as u64;
                reply.words[base + 1] = NetworkSocketKind::TcpStream as u32 as u64;
                reply.words[base + 2] = slot.state as u32 as u64;
                reply.words[base + 3] = ipv4_to_u32(slot.remote_address) as u64;
                reply.words[base + 4] = slot.remote_port as u64;
                reply.words[base + 5] = slot.local_port as u64;
                reply.words[base + 6] = ((slot.rx_bytes.min(u32::MAX as u64)) << 32)
                    | slot.tx_bytes.min(u32::MAX as u64);
                count += 1;
            }
            reply.word_count = 2 + count as u32 * 7;
            reply.words[1] = count as u64;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        _ => {}
    }

    Ok(())
}

pub(crate) fn handle_socket_request(
    log_handle: rt::Handle,
    config: NetworkConfig,
    sockets: &mut SocketSet<'_>,
    slot: &mut TcpTransportSlot,
    request: &RawMessage,
) -> rt::Result<()> {
    if request.handle_count < 1 {
        return Ok(());
    }
    let reply_handle = request.handles[0];
    let reply = match request.tag {
        x if x == NetworkSocketTag::StatusRequest as u32 => {
            let mut reply = RawMessage::empty(NetworkSocketTag::StatusReply as u32);
            reply.word_count = 8;
            reply.words[0] = NetworkStatus::Ok as u32 as u64;
            reply.words[1] = 0;
            reply.words[2] = NetworkSocketKind::TcpStream as u32 as u64;
            reply.words[3] = slot.state as u32 as u64;
            reply.words[4] = ipv4_to_u32(slot.remote_address) as u64;
            reply.words[5] = slot.remote_port as u64;
            reply.words[6] = slot.local_port as u64;
            reply.words[7] =
                ((slot.rx_bytes.min(u32::MAX as u64)) << 32) | slot.tx_bytes.min(u32::MAX as u64);
            reply
        }
        x if x == NetworkSocketTag::SendRequest as u32 => {
            let mut reply = RawMessage::empty(NetworkSocketTag::SendReply as u32);
            reply.word_count = 2;
            if request.word_count < 1 {
                reply.words[0] = NetworkStatus::InvalidTarget as u32 as u64;
            } else {
                let byte_len = request.words[0] as usize;
                let mut payload = [0u8; MAX_SOCKET_INLINE_BYTES];
                let payload = decode_inline_bytes(
                    &request.words[1..request.word_count as usize],
                    byte_len,
                    &mut payload,
                )?;
                if let Some(socket_handle) = slot.socket_handle {
                    let socket = sockets.get_mut::<tcp::Socket>(socket_handle);
                    if socket.may_send() && socket.can_send() {
                        match socket.send_slice(payload) {
                            Ok(written) => {
                                slot.last_activity_ticks =
                                    rt::monotonic_now().unwrap_or(slot.last_activity_ticks);
                                slot.tx_bytes = slot.tx_bytes.saturating_add(written as u64);
                                reply.words[0] = NetworkStatus::Ok as u32 as u64;
                                reply.words[1] = written as u64;
                            }
                            Err(_) => {
                                reply.words[0] = NetworkStatus::Busy as u32 as u64;
                                reply.words[1] = 0;
                            }
                        }
                    } else if socket.state() == tcp::State::Closed {
                        reply.words[0] = NetworkStatus::Closed as u32 as u64;
                        reply.words[1] = 0;
                    } else {
                        reply.words[0] = NetworkStatus::Busy as u32 as u64;
                        reply.words[1] = 0;
                    }
                } else {
                    reply.words[0] = NetworkStatus::Closed as u32 as u64;
                    reply.words[1] = 0;
                }
            }
            reply
        }
        x if x == NetworkSocketTag::ReceiveRequest as u32 => {
            let mut reply = RawMessage::empty(NetworkSocketTag::ReceiveReply as u32);
            reply.word_count = 2;
            let requested = request.words.first().copied().unwrap_or(0) as usize;
            let read_len = requested.min(MAX_SOCKET_INLINE_BYTES);
            let mut buffer = [0u8; MAX_SOCKET_INLINE_BYTES];
            if let Some(socket_handle) = slot.socket_handle {
                let socket = sockets.get_mut::<tcp::Socket>(socket_handle);
                if socket.can_recv() {
                    match socket.recv_slice(&mut buffer[..read_len]) {
                        Ok(count) => {
                            slot.last_activity_ticks =
                                rt::monotonic_now().unwrap_or(slot.last_activity_ticks);
                            slot.rx_bytes = slot.rx_bytes.saturating_add(count as u64);
                            reply.words[0] = NetworkStatus::Ok as u32 as u64;
                            reply.words[1] = count as u64;
                            let packed = pack_inline_bytes(&buffer[..count], &mut reply.words[2..])?;
                            reply.word_count = 2 + packed;
                        }
                        Err(_) => {
                            reply.words[0] = NetworkStatus::Busy as u32 as u64;
                            reply.words[1] = 0;
                        }
                    }
                } else if !socket.may_recv()
                    || rt::monotonic_now()
                        .unwrap_or(slot.last_activity_ticks)
                        .saturating_sub(slot.last_activity_ticks)
                        >= config.tcp_idle_timeout_ticks
                {
                    reply.words[0] = NetworkStatus::Closed as u32 as u64;
                    reply.words[1] = 0;
                } else {
                    reply.words[0] = NetworkStatus::Busy as u32 as u64;
                    reply.words[1] = 0;
                }
            } else {
                reply.words[0] = NetworkStatus::Closed as u32 as u64;
                reply.words[1] = 0;
            }
            reply
        }
        x if x == NetworkSocketTag::CloseRequest as u32 => {
            let mut reply = RawMessage::empty(NetworkSocketTag::CloseReply as u32);
            reply.word_count = 1;
            reply.words[0] = NetworkStatus::Ok as u32 as u64;
            close_transport_slot(log_handle, sockets, slot);
            reply
        }
        _ => return Ok(()),
    };

    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

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

pub(crate) fn update_transport_states(
    log_handle: rt::Handle,
    config: NetworkConfig,
    sockets: &mut SocketSet<'_>,
    transports: &mut [TcpTransportSlot; MAX_TCP_SOCKETS],
) {
    for slot in transports.iter_mut().filter(|slot| slot.active) {
        let Some(socket_handle) = slot.socket_handle else {
            continue;
        };
        let now = rt::monotonic_now().unwrap_or(slot.last_activity_ticks);
        let socket = sockets.get_mut::<tcp::Socket>(socket_handle);
        let raw_state = socket.state();
        if raw_state == tcp::State::Established && (socket.can_send() || socket.can_recv()) {
            slot.last_activity_ticks = now;
        }
        let mut state = socket_network_state(raw_state);
        if state == NetworkSocketState::Connecting
            && now.saturating_sub(slot.opened_at_ticks) >= config.tcp_connect_timeout_ticks
        {
            socket.abort();
            state = NetworkSocketState::Failed;
        }
        if slot.state != state {
            slot.state = state;
            if state == NetworkSocketState::Closed {
                let _ = emit_log(
                    log_handle,
                    LogSeverity::Info,
                    LogEvent::NetworkSocketClosed,
                    ipv4_to_u32(slot.remote_address) as u64,
                    slot.remote_port as u64,
                );
            }
        }
    }
}

fn socket_network_state(state: tcp::State) -> NetworkSocketState {
    match state {
        tcp::State::SynSent | tcp::State::SynReceived => NetworkSocketState::Connecting,
        tcp::State::Established | tcp::State::CloseWait => NetworkSocketState::Established,
        tcp::State::FinWait1
        | tcp::State::FinWait2
        | tcp::State::Closing
        | tcp::State::LastAck
        | tcp::State::TimeWait => NetworkSocketState::Closing,
        tcp::State::Listen => NetworkSocketState::Connecting,
        tcp::State::Closed => NetworkSocketState::Closed,
    }
}

pub(crate) fn close_transport_slot(
    log_handle: rt::Handle,
    sockets: &mut SocketSet<'_>,
    slot: &mut TcpTransportSlot,
) {
    if let Some(socket_handle) = slot.socket_handle {
        sockets.get_mut::<tcp::Socket>(socket_handle).abort();
    }
    let _ = emit_log(
        log_handle,
        LogSeverity::Info,
        LogEvent::NetworkSocketClosed,
        ipv4_to_u32(slot.remote_address) as u64,
        slot.remote_port as u64,
    );
    if slot.control_handle != rt::INVALID_HANDLE {
        let _ = rt::handle_close(slot.control_handle);
    }
    *slot = TcpTransportSlot::empty();
}

fn allocate_transport_slot(transports: &[TcpTransportSlot; MAX_TCP_SOCKETS]) -> Option<usize> {
    transports.iter().position(|slot| !slot.active)
}

fn allocate_ephemeral_port(next_local_port: &mut u16) -> u16 {
    let current = *next_local_port;
    *next_local_port = if *next_local_port >= u16::MAX - 1 {
        EPHEMERAL_PORT_BASE
    } else {
        next_local_port.saturating_add(1)
    };
    current
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
