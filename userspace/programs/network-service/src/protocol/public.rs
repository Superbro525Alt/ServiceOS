use smoltcp::{
    iface::{Interface, SocketHandle, SocketSet},
    socket::tcp,
    wire::{IpAddress, Ipv4Address},
};

use serviceos_userspace_runtime as rt;
use rt::{
    LogEvent, LogSeverity, NetworkConfigState, NetworkSocketKind, NetworkSocketState,
    NetworkStatus, NetworkTag, RawMessage,
};

use crate::{
    consts::{EPHEMERAL_PORT_BASE, MAX_HOSTNAME_BYTES, MAX_TCP_SOCKETS},
    device::KernelPacketDevice,
    types::{
        HostEntry, InterfaceRuntimeState, NetworkConfig, TcpListenerSlot, TcpTransportSlot,
        UdpDatagramSlot,
    },
    util::{decode_inline_text, emit_log, ipv4_to_u32},
};

use super::{
    listeners::open_listener,
    transport::{perform_ping, resolve_target},
    udp::open_udp_socket,
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
    udp_slots: &mut [UdpDatagramSlot; crate::consts::MAX_UDP_SOCKETS],
    udp_handles: [SocketHandle; crate::consts::MAX_UDP_SOCKETS],
    listeners: &mut [TcpListenerSlot; crate::consts::MAX_TCP_LISTENERS],
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
            if runtime_state.state == NetworkConfigState::Pending && crate::config::parse_ipv4(target).is_none() {
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
            let routed = open_udp_socket(
                request,
                log_handle,
                udp_slots,
                udp_handles,
                sockets,
                next_local_port,
            )?;
            if !routed {
                handle_socket_open_request(
                    request,
                    log_handle,
                    config,
                    runtime_state,
                    hosts,
                    iface,
                    device,
                    sockets,
                    dns_handle,
                    transports,
                    tcp_handles,
                    next_local_port,
                )?;
            }
        }
        x if x == NetworkTag::SocketListenRequest as u32 => {
            open_listener(request, log_handle, listeners, transports, tcp_handles, sockets)?;
        }
        x if x == NetworkTag::SocketListRequest as u32 => {
            if request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(NetworkTag::SocketListReply as u32);
            reply.words[0] = NetworkStatus::Ok as u32 as u64;
            let mut count = 0usize;
            let push_entry = |reply: &mut RawMessage, count: &mut usize, entry: [u64; 7]| {
                if 2 + (*count + 1) * 7 > rt::IPC_MAX_WORDS {
                    return;
                }
                let base = 2 + *count * 7;
                reply.words[base] = entry[0];
                reply.words[base + 1] = entry[1];
                reply.words[base + 2] = entry[2];
                reply.words[base + 3] = entry[3];
                reply.words[base + 4] = entry[4];
                reply.words[base + 5] = entry[5];
                reply.words[base + 6] = entry[6];
                *count += 1;
            };
            for (index, slot) in transports.iter().filter(|slot| slot.active).enumerate() {
                push_entry(
                    &mut reply,
                    &mut count,
                    [
                        index as u64,
                        NetworkSocketKind::TcpStream as u32 as u64,
                        slot.state as u32 as u64,
                        ipv4_to_u32(slot.remote_address) as u64,
                        slot.remote_port as u64,
                        slot.local_port as u64,
                        ((slot.rx_bytes.min(u32::MAX as u64)) << 32)
                            | slot.tx_bytes.min(u32::MAX as u64),
                    ],
                );
            }
            for slot in listeners.iter().filter(|slot| slot.active) {
                let entry_slot = count;
                push_entry(
                    &mut reply,
                    &mut count,
                    [
                        entry_slot as u64,
                        NetworkSocketKind::TcpStream as u32 as u64,
                        NetworkSocketState::Connecting as u32 as u64,
                        0,
                        0,
                        slot.local_port as u64,
                        slot.accept_len as u64,
                    ],
                );
            }
            for slot in udp_slots.iter().filter(|slot| slot.active) {
                let entry_slot = count;
                push_entry(
                    &mut reply,
                    &mut count,
                    [
                        entry_slot as u64,
                        NetworkSocketKind::UdpDatagram as u32 as u64,
                        NetworkSocketState::Established as u32 as u64,
                        0,
                        0,
                        slot.local_port as u64,
                        ((slot.rx_bytes.min(u32::MAX as u64)) << 32)
                            | slot.tx_bytes.min(u32::MAX as u64),
                    ],
                );
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

#[allow(clippy::too_many_arguments)]
fn handle_socket_open_request(
    request: &RawMessage,
    log_handle: rt::Handle,
    config: NetworkConfig,
    runtime_state: InterfaceRuntimeState,
    hosts: &[HostEntry],
    iface: &mut Interface,
    device: &mut KernelPacketDevice,
    sockets: &mut SocketSet<'_>,
    dns_handle: SocketHandle,
    transports: &mut [TcpTransportSlot; MAX_TCP_SOCKETS],
    tcp_handles: [SocketHandle; MAX_TCP_SOCKETS],
    next_local_port: &mut u16,
) -> rt::Result<()> {
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
    Ok(())
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
