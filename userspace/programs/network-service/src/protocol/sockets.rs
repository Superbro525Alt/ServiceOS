use smoltcp::{
    iface::SocketSet,
    socket::tcp,
};

use serviceos_userspace_runtime as rt;
use rt::{
    LogEvent, LogSeverity, NetworkSocketKind, NetworkSocketState, NetworkSocketTag,
    NetworkStatus, RawMessage,
};

use crate::{
    consts::{MAX_SOCKET_INLINE_BYTES, MAX_TCP_SOCKETS},
    types::{NetworkConfig, TcpTransportSlot},
    util::{decode_inline_bytes, emit_log, ipv4_to_u32, pack_inline_bytes},
};

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
