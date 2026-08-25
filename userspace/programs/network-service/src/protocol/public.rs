use smoltcp::{
    iface::{Interface, SocketHandle, SocketSet},
    socket::tcp,
    wire::{IpAddress, Ipv4Address},
};

use rt::{
    LogEvent, LogSeverity, NetworkConfigState, NetworkSocketKind, NetworkSocketState,
    NetworkStatus, NetworkTag, RawMessage,
};
use serviceos_userspace_runtime as rt;

use crate::{
    cache::ResolverCache,
    consts::{
        FIREWALL_RULES_GET_REQUEST, FIREWALL_RULES_REPLY, FIREWALL_RULES_SET_REQUEST,
        MAX_HOSTNAME_BYTES, MAX_TCP_SOCKETS, RESOLVE_EX_REQUEST, RESOLVE_EX_TYPE_A,
        RESOLVE_EX_TYPE_AAAA, RESOLVE_EX_TYPE_TXT,
    },
    device::KernelPacketDevice,
    dnsmsg::QueryType,
    dnsresolv::{self, ChaseDetail},
    firewall::{Direction, FirewallState, Proto},
    types::{
        HostEntry, InterfaceRuntimeState, NetworkConfig, TcpListenerSlot, TcpTransportSlot,
        UdpDatagramSlot,
    },
    util::{decode_inline_text, emit_log, ipv4_to_u32, pack_inline_bytes, ticks_to_millis},
};

use super::{listeners::open_listener, transport::perform_ping, udp::open_udp_socket};

fn status_for_detail(detail: ChaseDetail) -> NetworkStatus {
    match detail {
        ChaseDetail::Fresh | ChaseDetail::PositiveCache => NetworkStatus::Ok,
        ChaseDetail::NegativeCache
        | ChaseDetail::NxDomain
        | ChaseDetail::NoData
        | ChaseDetail::ChainTooLong => NetworkStatus::NotFound,
        ChaseDetail::ServFail => NetworkStatus::Busy,
        ChaseDetail::Timeout => NetworkStatus::Timeout,
        ChaseDetail::Malformed => NetworkStatus::InvalidTarget,
    }
}

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
    dns_client_handle: SocketHandle,
    icmp_handle: SocketHandle,
    next_sequence: &mut u16,
    next_query_id: &mut u16,
    resolver_cache: &mut ResolverCache,
    firewall: &mut FirewallState,
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
            reply.word_count = 16;
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
                // Resolver cache statistics ride in the trailing word:
                // hits in the high half, misses in the low half. The full
                // firewall table (rules, counters, default policy) is
                // queryable via FirewallRulesGetRequest.
                let now_ms = ticks_to_millis(rt::monotonic_now().unwrap_or(0));
                resolver_cache.prune(now_ms);
                reply.words[15] = (resolver_cache.hits.min(u32::MAX as u64) << 32)
                    | resolver_cache.misses.min(u32::MAX as u64);
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
            reply.word_count = 4;
            if runtime_state.state == NetworkConfigState::Pending
                && crate::config::parse_ipv4(target).is_none()
            {
                reply.words[0] = NetworkStatus::Busy as u32 as u64;
                reply.words[1] = 0;
                reply.words[2] = 0;
                reply.words[3] = crate::consts::RESOLVE_DETAIL_TIMEOUT;
            } else {
                match dnsresolv::resolve_ipv4(
                    target,
                    hosts,
                    resolver_cache,
                    config.dns_query_timeout_ticks,
                    iface,
                    device,
                    sockets,
                    dns_client_handle,
                    next_query_id,
                    runtime_state.dns_server,
                ) {
                    Ok(outcome) => {
                        let status = status_for_detail(outcome.detail);
                        let found = outcome.detail.is_success();
                        reply.words[0] = status as u32 as u64;
                        reply.words[1] = found as u64;
                        reply.words[2] = outcome.address.unwrap_or(0) as u64;
                        reply.words[3] = outcome.detail.word();
                        if found {
                            let _ = emit_log(
                                log_handle,
                                LogSeverity::Debug,
                                LogEvent::NetworkResolveCompleted,
                                outcome.address.unwrap_or(0) as u64,
                                outcome.detail.word(),
                            );
                        }
                    }
                    Err(_) => {
                        reply.words[0] = NetworkStatus::Busy as u32 as u64;
                        reply.words[1] = 0;
                        reply.words[2] = 0;
                        reply.words[3] = crate::consts::RESOLVE_DETAIL_TIMEOUT;
                    }
                }
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == RESOLVE_EX_REQUEST => {
            if request.word_count < 2 || request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut text = [0u8; MAX_HOSTNAME_BYTES];
            let target = decode_inline_text(
                &request.words[2..request.word_count as usize],
                request.words[0] as usize,
                &mut text,
            )?;
            let qtype = match request.words[1] {
                RESOLVE_EX_TYPE_A => Some(QueryType::A),
                RESOLVE_EX_TYPE_AAAA => Some(QueryType::Aaaa),
                RESOLVE_EX_TYPE_TXT => Some(QueryType::Txt),
                _ => None,
            };
            let mut reply = RawMessage::empty(crate::consts::RESOLVE_EX_REPLY as u32);
            reply.words[0] = NetworkStatus::InvalidTarget as u32 as u64;
            reply.words[1] = crate::consts::RESOLVE_DETAIL_MALFORMED;
            reply.words[2] = 0;
            reply.word_count = 3;
            if let Some(qtype) = qtype {
                let outcome = if runtime_state.state == NetworkConfigState::Pending
                    && crate::config::parse_ipv4(target).is_none()
                {
                    Ok(dnsresolv::ChaseOutcome::with(ChaseDetail::Timeout))
                } else {
                    dnsresolv::resolve_typed(
                        target,
                        qtype,
                        hosts,
                        resolver_cache,
                        config.dns_query_timeout_ticks,
                        iface,
                        device,
                        sockets,
                        dns_client_handle,
                        next_query_id,
                        runtime_state.dns_server,
                    )
                };
                if let Ok(outcome) = outcome {
                    let mut payload = [0u8; 64];
                    let payload_len = match qtype {
                        QueryType::A if outcome.address.is_some() => {
                            payload[..4]
                                .copy_from_slice(&outcome.address.unwrap_or(0).to_be_bytes());
                            4
                        }
                        QueryType::Aaaa => {
                            let count = outcome.aaaa_count.max(if outcome.detail.is_success() {
                                1
                            } else {
                                0
                            });
                            let count = count.min(outcome.aaaa.len());
                            for (index, address) in outcome.aaaa.iter().take(count).enumerate() {
                                payload[index * 16..index * 16 + 16].copy_from_slice(address);
                            }
                            count * 16
                        }
                        QueryType::Txt => {
                            let len = outcome.txt_len.min(payload.len());
                            payload[..len].copy_from_slice(&outcome.txt[..len]);
                            len
                        }
                        _ => 0,
                    };
                    reply.words[0] = status_for_detail(outcome.detail) as u32 as u64;
                    reply.words[1] = outcome.detail.word();
                    if payload_len > 0 {
                        let packed =
                            pack_inline_bytes(&payload[..payload_len], &mut reply.words[3..])?;
                        reply.words[2] = packed as u64;
                        reply.word_count = 3 + packed;
                    } else {
                        reply.word_count = 3;
                    }
                } else {
                    reply.words[0] = NetworkStatus::Busy as u32 as u64;
                    reply.word_count = 3;
                }
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == FIREWALL_RULES_SET_REQUEST => {
            if request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let op = request.words.first().copied().unwrap_or(u64::MAX);
            let mut reply = RawMessage::empty(FIREWALL_RULES_REPLY as u32);
            reply.words[0] = NetworkStatus::Ok as u32 as u64;
            let applied = match op {
                0 => {
                    let count = request.words.get(1).copied().unwrap_or(0) as usize;
                    let fields = &request.words[2..request.word_count as usize];
                    firewall.replace_all(fields, count).is_some()
                }
                1 => {
                    firewall
                        .set_default_inbound_allow(request.words.get(1).copied().unwrap_or(1) != 0);
                    true
                }
                2 => {
                    firewall.clear_rules();
                    true
                }
                _ => false,
            };
            if applied {
                reply.word_count = firewall.encode_reply(&mut reply.words) as u32 + 1;
                let _ = rt::write_logf(
                    "network",
                    format_args!(
                        "firewall state rules={} default-inbound={}",
                        firewall.rule_count,
                        if firewall.default_inbound_allow {
                            "allow"
                        } else {
                            "deny"
                        }
                    ),
                );
            } else {
                reply.word_count = 1;
                reply.words[0] = NetworkStatus::InvalidTarget as u32 as u64;
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == FIREWALL_RULES_GET_REQUEST => {
            if request.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = request.handles[0];
            let mut reply = RawMessage::empty(FIREWALL_RULES_REPLY as u32);
            reply.words[0] = NetworkStatus::Ok as u32 as u64;
            reply.word_count = firewall.encode_reply(&mut reply.words) as u32 + 1;
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
                match dnsresolv::resolve_ipv4(
                    target,
                    hosts,
                    resolver_cache,
                    config.dns_query_timeout_ticks,
                    iface,
                    device,
                    sockets,
                    dns_client_handle,
                    next_query_id,
                    runtime_state.dns_server,
                )? {
                    outcome if outcome.detail.is_success() => {
                        let address = ipv4_from_word(outcome.address.unwrap_or(0));
                        if !firewall.decide(Direction::Outbound, Proto::Icmp, 0, 0) {
                            let _ = rt::write_logf(
                                "network",
                                format_args!(
                                    "firewall deny outbound icmp target={}",
                                    outcome.address.unwrap_or(0)
                                ),
                            );
                            reply.words[0] = NetworkStatus::Denied as u32 as u64;
                            reply.words[1] = outcome.address.unwrap_or(0) as u64;
                            reply.words[2] = 0;
                        } else {
                            match perform_ping(
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
                            }
                        }
                    }
                    outcome => {
                        reply.words[0] = status_for_detail(outcome.detail) as u32 as u64;
                        reply.words[1] = outcome.address.unwrap_or(0) as u64;
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
                    dns_client_handle,
                    next_query_id,
                    resolver_cache,
                    firewall,
                    transports,
                    tcp_handles,
                    next_local_port,
                )?;
            }
        }
        x if x == NetworkTag::SocketListenRequest as u32 => {
            open_listener(
                request,
                log_handle,
                listeners,
                transports,
                tcp_handles,
                sockets,
            )?;
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

fn ipv4_from_word(word: u32) -> Ipv4Address {
    crate::util::u32_to_ipv4(word)
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
    dns_client_handle: SocketHandle,
    next_query_id: &mut u16,
    resolver_cache: &mut ResolverCache,
    firewall: &mut FirewallState,
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
    } else {
        match dnsresolv::resolve_ipv4(
            target,
            hosts,
            resolver_cache,
            config.dns_query_timeout_ticks,
            iface,
            device,
            sockets,
            dns_client_handle,
            next_query_id,
            runtime_state.dns_server,
        )? {
            outcome if outcome.detail.is_success() => {
                let remote_address = ipv4_from_word(outcome.address.unwrap_or(0));
                if !firewall.decide(Direction::Outbound, Proto::Tcp, 0, remote_port) {
                    let _ = rt::write_logf(
                        "network",
                        format_args!(
                            "firewall deny outbound tcp {}:{}",
                            outcome.address.unwrap_or(0),
                            remote_port
                        ),
                    );
                    reply.words[0] = NetworkStatus::Denied as u32 as u64;
                } else if let Some(slot_index) = allocate_transport_slot(transports) {
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
            }
            outcome => {
                reply.words[0] = status_for_detail(outcome.detail) as u32 as u64;
            }
        }
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
        crate::consts::EPHEMERAL_PORT_BASE
    } else {
        next_local_port.saturating_add(1)
    };
    current
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_mapping_covers_details() {
        assert_eq!(status_for_detail(ChaseDetail::Fresh), NetworkStatus::Ok);
        assert_eq!(
            status_for_detail(ChaseDetail::NxDomain),
            NetworkStatus::NotFound
        );
        assert_eq!(
            status_for_detail(ChaseDetail::ServFail),
            NetworkStatus::Busy
        );
        assert_eq!(
            status_for_detail(ChaseDetail::Timeout),
            NetworkStatus::Timeout
        );
        assert_eq!(
            status_for_detail(ChaseDetail::Malformed),
            NetworkStatus::InvalidTarget
        );
    }

    #[test]
    fn detail_words_are_stable() {
        assert_eq!(
            crate::consts::RESOLVE_DETAIL_POSITIVE_CACHE,
            ChaseDetail::PositiveCache.word()
        );
        assert_eq!(
            crate::consts::RESOLVE_DETAIL_FRESH,
            ChaseDetail::Fresh.word()
        );
    }
}
