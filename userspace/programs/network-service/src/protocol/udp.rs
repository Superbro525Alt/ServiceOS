use smoltcp::{
    iface::{SocketHandle, SocketSet},
    socket::udp,
    wire::{IpAddress, Ipv4Address},
};

use rt::{
    LogEvent, LogSeverity, NetworkSocketKind, NetworkSocketState, NetworkSocketTag, NetworkStatus,
    NetworkTag, RawMessage,
};
use serviceos_userspace_runtime as rt;

use crate::{
    consts::{EPHEMERAL_PORT_BASE, MAX_SOCKET_INLINE_BYTES, MAX_UDP_SOCKETS},
    firewall::{Direction, FirewallState, Proto},
    types::UdpDatagramSlot,
    util::{decode_inline_bytes, emit_log, ipv4_to_u32, pack_inline_bytes},
};

/// Handle SocketOpenRequest for kind=UdpDatagram: words[1] carries
/// ((target_len << 16) | local_port); an empty target with port 0 selects the
/// next ephemeral port. On success the reply carries the datagram control
/// handle (SendTo/ReceiveFrom/Bind/Status/Close protocol).
pub(crate) fn open_udp_socket(
    request: &RawMessage,
    log_handle: rt::Handle,
    udp_slots: &mut [UdpDatagramSlot; MAX_UDP_SOCKETS],
    udp_handles: [SocketHandle; MAX_UDP_SOCKETS],
    sockets: &mut SocketSet<'_>,
    next_local_port: &mut u16,
) -> rt::Result<bool> {
    if request.word_count < 2 || request.handle_count < 1 {
        return Ok(false);
    }
    if request.words[0] as u32 != NetworkSocketKind::UdpDatagram as u32 {
        return Ok(false);
    }
    let reply_handle = request.handles[0];
    let requested_port = (request.words[1] & 0xffff) as u16;
    let mut reply = RawMessage::empty(NetworkTag::SocketOpenReply as u32);
    reply.word_count = 1;

    let Some(slot_index) = udp_slots.iter().position(|slot| !slot.active) else {
        reply.words[0] = NetworkStatus::CapacityExceeded as u32 as u64;
        let _ = rt::channel_send(reply_handle, &reply);
        let _ = rt::handle_close(reply_handle);
        return Ok(true);
    };
    // Datagrams use pre-added idle pool sockets; binding claims the slot.
    let socket_handle = udp_handles[slot_index];

    let session = rt::channel_create()?;
    let local_port = if requested_port == 0 {
        allocate_ephemeral_port(next_local_port)
    } else {
        requested_port
    };
    {
        let socket = sockets.get_mut::<udp::Socket>(socket_handle);
        if socket.is_open() {
            socket.close();
        }
    }
    let bind_result = sockets
        .get_mut::<udp::Socket>(socket_handle)
        .bind(local_port)
        .is_ok();

    if !bind_result {
        let _ = rt::handle_close(session.first);
        let _ = rt::handle_close(session.second);
        reply.words[0] = NetworkStatus::InvalidTarget as u32 as u64;
        let _ = rt::channel_send(reply_handle, &reply);
        let _ = rt::handle_close(reply_handle);
        return Ok(true);
    }

    udp_slots[slot_index] = UdpDatagramSlot {
        active: true,
        control_handle: session.first,
        socket_handle: Some(socket_handle),
        local_port,
        rx_bytes: 0,
        tx_bytes: 0,
        last_activity_ticks: rt::monotonic_now().unwrap_or(0),
    };
    reply.words[0] = NetworkStatus::Ok as u32 as u64;
    reply.handle_count = 1;
    reply.handles[0] = session.second;
    reply.handle_rights[0] = rt::rights::SEND | rt::rights::RECEIVE;
    let _ = emit_log(
        log_handle,
        LogSeverity::Info,
        LogEvent::NetworkSocketOpened,
        ipv4_to_u32(Ipv4Address::UNSPECIFIED) as u64,
        local_port as u64,
    );
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(session.second);
    let _ = rt::handle_close(reply_handle);
    Ok(true)
}

pub(crate) fn close_udp_slot(sockets: &mut SocketSet<'_>, slot: &mut UdpDatagramSlot) {
    if let Some(socket_handle) = slot.socket_handle.take() {
        let socket = sockets.get_mut::<udp::Socket>(socket_handle);
        socket.close();
    }
    if slot.control_handle != rt::INVALID_HANDLE {
        let _ = rt::handle_close(slot.control_handle);
    }
    *slot = UdpDatagramSlot::empty();
}

pub(crate) fn handle_datagram_request(
    log_handle: rt::Handle,
    sockets: &mut SocketSet<'_>,
    slot: &mut UdpDatagramSlot,
    request: &RawMessage,
    firewall: &mut FirewallState,
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
            reply.words[2] = NetworkSocketKind::UdpDatagram as u32 as u64;
            reply.words[3] = NetworkSocketState::Established as u32 as u64;
            reply.words[4] = 0;
            reply.words[5] = 0;
            reply.words[6] = slot.local_port as u64;
            reply.words[7] =
                (slot.rx_bytes.min(u32::MAX as u64)) << 32 | slot.tx_bytes.min(u32::MAX as u64);
            reply
        }
        x if x == NetworkSocketTag::BindRequest as u32 => {
            let mut reply = RawMessage::empty(NetworkSocketTag::BindReply as u32);
            reply.word_count = 1;
            let port = request.words.first().copied().unwrap_or(0) as u16;
            match (slot.socket_handle, port) {
                (None, _) => {
                    reply.words[0] = NetworkStatus::Closed as u32 as u64;
                }
                (_, 0) => {
                    reply.words[0] = NetworkStatus::InvalidTarget as u32 as u64;
                }
                (Some(socket_handle), _) => {
                    let bound = sockets
                        .get_mut::<udp::Socket>(socket_handle)
                        .bind(port)
                        .is_ok();
                    if bound {
                        slot.local_port = port;
                        reply.words[0] = NetworkStatus::Ok as u32 as u64;
                    } else {
                        reply.words[0] = NetworkStatus::InvalidTarget as u32 as u64;
                    }
                }
            }
            reply
        }
        x if x == NetworkSocketTag::SendToRequest as u32 => {
            send_datagram(sockets, slot, request, firewall)
        }
        x if x == NetworkSocketTag::ReceiveRequest as u32
            || x == NetworkSocketTag::ReceiveFromRequest as u32 =>
        {
            let reply = receive_datagram(sockets, slot, request, firewall)?;
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
            return Ok(());
        }
        x if x == NetworkSocketTag::CloseRequest as u32 => {
            let _ = emit_log(
                log_handle,
                LogSeverity::Info,
                LogEvent::NetworkSocketClosed,
                0,
                slot.local_port as u64,
            );
            close_udp_slot(sockets, slot);
            // Slot is cleared; nothing to reply to on a closed channel.
            return Ok(());
        }
        _ => return Ok(()),
    };
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

fn send_datagram(
    sockets: &mut SocketSet<'_>,
    slot: &mut UdpDatagramSlot,
    request: &RawMessage,
    firewall: &mut FirewallState,
) -> RawMessage {
    let mut reply = RawMessage::empty(NetworkSocketTag::SendToReply as u32);
    reply.word_count = 2;
    if request.word_count < 2 {
        reply.words[0] = NetworkStatus::InvalidTarget as u32 as u64;
        reply.words[1] = 0;
        return reply;
    }
    let byte_len = request.words[0] as usize;
    let packed_endpoint = request.words[1];
    let address_be = (packed_endpoint >> 16) as u32;
    let port = packed_endpoint as u16;
    let octets = address_be.to_be_bytes();
    let endpoint = smoltcp::wire::IpEndpoint {
        addr: smoltcp::wire::IpAddress::Ipv4(Ipv4Address::new(
            octets[0], octets[1], octets[2], octets[3],
        )),
        port,
    };
    let mut payload = [0u8; MAX_SOCKET_INLINE_BYTES];
    let payload = match decode_inline_bytes(
        &request.words[2..request.word_count as usize],
        byte_len,
        &mut payload,
    ) {
        Ok(payload) => payload,
        Err(_) => {
            reply.words[0] = NetworkStatus::InvalidTarget as u32 as u64;
            reply.words[1] = 0;
            return reply;
        }
    };
    let Some(socket_handle) = slot.socket_handle else {
        reply.words[0] = NetworkStatus::Closed as u32 as u64;
        reply.words[1] = 0;
        return reply;
    };
    if !firewall.decide(Direction::Outbound, Proto::Udp, slot.local_port, port) {
        let _ = rt::write_logf(
            "network",
            format_args!(
                "firewall deny outbound udp local={} remote={}:{}",
                slot.local_port,
                crate::util::ipv4_to_u32(crate::util::u32_to_ipv4(address_be)),
                port
            ),
        );
        reply.words[0] = NetworkStatus::Denied as u32 as u64;
        reply.words[1] = 0;
        return reply;
    }
    let socket = sockets.get_mut::<udp::Socket>(socket_handle);
    match socket.send_slice(payload, endpoint) {
        Ok(()) => {
            slot.last_activity_ticks = rt::monotonic_now().unwrap_or(slot.last_activity_ticks);
            slot.tx_bytes = slot.tx_bytes.saturating_add(payload.len() as u64);
            reply.words[0] = NetworkStatus::Ok as u32 as u64;
            reply.words[1] = payload.len() as u64;
        }
        Err(_) => {
            reply.words[0] = NetworkStatus::Busy as u32 as u64;
            reply.words[1] = 0;
        }
    }
    reply
}

fn receive_datagram(
    sockets: &mut SocketSet<'_>,
    slot: &mut UdpDatagramSlot,
    request: &RawMessage,
    firewall: &mut FirewallState,
) -> rt::Result<RawMessage> {
    let mut reply = RawMessage::empty(NetworkSocketTag::ReceiveFromReply as u32);
    reply.word_count = 3;
    let requested = request.words.first().copied().unwrap_or(0) as usize;
    let read_len = requested.min(MAX_SOCKET_INLINE_BYTES);
    let mut buffer = [0u8; MAX_SOCKET_INLINE_BYTES];
    let Some(socket_handle) = slot.socket_handle else {
        reply.words[0] = NetworkStatus::Closed as u32 as u64;
        reply.words[1] = 0;
        reply.words[2] = 0;
        return Ok(reply);
    };
    let socket = sockets.get_mut::<udp::Socket>(socket_handle);
    match socket.recv_slice(&mut buffer[..read_len]) {
        Ok((count, metadata)) => {
            let IpAddress::Ipv4(remote) = metadata.endpoint.addr;
            let source_be = u32::from_be_bytes(remote.octets());
            if !firewall.decide(Direction::Inbound, Proto::Udp, slot.local_port, metadata.endpoint.port)
            {
                // Consume-and-drop: the datagram is counted as filtered.
                let _ = rt::write_logf(
                    "network",
                    format_args!(
                        "firewall deny inbound udp local={} remote={}:{}",
                        slot.local_port,
                        source_be,
                        metadata.endpoint.port
                    ),
                );
                reply.words[0] = NetworkStatus::Busy as u32 as u64;
                reply.words[1] = 0;
                reply.words[2] = 0;
                return Ok(reply);
            }
            slot.last_activity_ticks = rt::monotonic_now().unwrap_or(slot.last_activity_ticks);
            slot.rx_bytes = slot.rx_bytes.saturating_add(count as u64);
            reply.words[0] = NetworkStatus::Ok as u32 as u64;
            reply.words[1] = count as u64;
            reply.words[2] = rt::pack_ipv4_endpoint(source_be, metadata.endpoint.port);
            let packed = pack_inline_bytes(&buffer[..count], &mut reply.words[3..])?;
            reply.word_count = 3 + packed;
        }
        Err(_) => {
            // Nonblocking contract: no queued datagram -> Busy.
            reply.words[0] = NetworkStatus::Busy as u32 as u64;
            reply.words[1] = 0;
            reply.words[2] = 0;
        }
    }
    Ok(reply)
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
