use smoltcp::{
    iface::{SocketHandle, SocketSet},
    socket::tcp,
};

use rt::{
    LogEvent, LogSeverity, NetworkSocketKind, NetworkSocketState, NetworkSocketTag, NetworkStatus,
    NetworkTag, RawMessage,
};
use serviceos_userspace_runtime as rt;

use crate::{
    consts::MAX_TCP_LISTENERS,
    firewall::{Direction, FirewallState, Proto, RemoteAddress},
    types::{TcpListenerSlot, TcpTransportSlot},
    util::{emit_log, ipv4_to_u32},
};

/// Listener/pool invariant: a listener only ever drives a pool socket
/// (`tcp_handles[j]`) whose transport slot `transports[j]` is inactive.
/// Inactive slots never touch their socket. When an inbound handshake lands
/// on that handle the connection is adopted by the matching transport slot
/// and the listener goes to pending-re-arm until another slot frees up.
/// Handle SocketListenRequest: words[0] = NetworkSocketKind (TcpStream),
/// words[1] = pack_listen_params(local_port, backlog). The reply carries a
/// listener control handle speaking Status/Accept/Close.
pub(crate) fn open_listener(
    request: &RawMessage,
    log_handle: rt::Handle,
    listeners: &mut [TcpListenerSlot; MAX_TCP_LISTENERS],
    transports: &[TcpTransportSlot; crate::consts::MAX_TCP_SOCKETS],
    tcp_handles: [SocketHandle; crate::consts::MAX_TCP_SOCKETS],
    sockets: &mut SocketSet<'_>,
) -> rt::Result<bool> {
    if request.word_count < 2 || request.handle_count < 1 {
        return Ok(false);
    }
    if request.words[0] as u32 != NetworkSocketKind::TcpStream as u32 {
        return Ok(false);
    }
    let reply_handle = request.handles[0];
    let (local_port, backlog) = rt::unpack_listen_params(request.words[1]);
    let mut reply = RawMessage::empty(NetworkTag::SocketListenReply as u32);
    reply.word_count = 1;

    let Some(slot_index) = listeners.iter().position(|slot| !slot.active) else {
        reply.words[0] = NetworkStatus::CapacityExceeded as u32 as u64;
        send_reply(reply_handle, reply);
        return Ok(true);
    };
    if local_port == 0 {
        reply.words[0] = NetworkStatus::InvalidTarget as u32 as u64;
        send_reply(reply_handle, reply);
        return Ok(true);
    }
    let Some(socket_handle) = free_pool_handle(listeners, transports, tcp_handles) else {
        reply.words[0] = NetworkStatus::CapacityExceeded as u32 as u64;
        send_reply(reply_handle, reply);
        return Ok(true);
    };

    let session = rt::channel_create()?;
    let listening = {
        let socket = sockets.get_mut::<tcp::Socket>(socket_handle);
        if socket.is_open() {
            socket.abort();
        }
        socket.listen(local_port).is_ok()
    };
    if !listening {
        let _ = rt::handle_close(session.first);
        let _ = rt::handle_close(session.second);
        reply.words[0] = NetworkStatus::InvalidTarget as u32 as u64;
        send_reply(reply_handle, reply);
        return Ok(true);
    }

    listeners[slot_index] = TcpListenerSlot {
        active: true,
        control_handle: session.first,
        socket_handle: Some(socket_handle),
        local_port,
        backlog: backlog.max(1),
        ..TcpListenerSlot::empty()
    };
    reply.words[0] = NetworkStatus::Ok as u32 as u64;
    reply.handle_count = 1;
    reply.handles[0] = session.second;
    reply.handle_rights[0] = rt::rights::SEND | rt::rights::RECEIVE;
    let _ = emit_log(
        log_handle,
        LogSeverity::Info,
        LogEvent::NetworkSocketOpened,
        0,
        local_port as u64,
    );
    send_reply(reply_handle, reply);
    Ok(true)
}

fn send_reply(reply_handle: rt::Handle, reply: RawMessage) {
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
}

/// Same-service internal listener bind: the calling service lives inside this
/// process, so no IPC reply is needed and the control channel half is
/// retained internally (the client half is closed immediately). Keeps the
/// pool/listener invariants identical to `open_listener` without any
/// RawMessage traffic (and thus without the stack cost of IPC buffers).
pub(crate) fn open_internal_listener(
    log_handle: rt::Handle,
    listeners: &mut [TcpListenerSlot; MAX_TCP_LISTENERS],
    transports: &[TcpTransportSlot; crate::consts::MAX_TCP_SOCKETS],
    tcp_handles: [SocketHandle; crate::consts::MAX_TCP_SOCKETS],
    sockets: &mut SocketSet<'_>,
    local_port: u16,
    backlog: u32,
) -> bool {
    let Some(slot_index) = listeners.iter().position(|slot| !slot.active) else {
        return false;
    };
    let Some(socket_handle) = free_pool_handle(listeners, transports, tcp_handles) else {
        return false;
    };
    let listening = {
        let socket = sockets.get_mut::<tcp::Socket>(socket_handle);
        if socket.is_open() {
            socket.abort();
        }
        socket.listen(local_port).is_ok()
    };
    if !listening {
        return false;
    }
    let session = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return false,
    };
    listeners[slot_index] = TcpListenerSlot {
        active: true,
        control_handle: session.first,
        socket_handle: Some(socket_handle),
        local_port,
        backlog: backlog.max(1),
        ..TcpListenerSlot::empty()
    };
    // The external client half is meaningless on the internal path.
    let _ = rt::handle_close(session.second);
    let _ = emit_log(
        log_handle,
        LogSeverity::Info,
        LogEvent::NetworkSocketOpened,
        0,
        local_port as u64,
    );
    true
}

fn free_pool_handle(
    listeners: &[TcpListenerSlot; MAX_TCP_LISTENERS],
    transports: &[TcpTransportSlot; crate::consts::MAX_TCP_SOCKETS],
    tcp_handles: [SocketHandle; crate::consts::MAX_TCP_SOCKETS],
) -> Option<SocketHandle> {
    for (index, transport) in transports.iter().enumerate() {
        if transport.active {
            continue;
        }
        let held_by_listener = listeners
            .iter()
            .any(|listener| listener.active && listener.socket_handle == Some(tcp_handles[index]));
        if !held_by_listener {
            return Some(tcp_handles[index]);
        }
    }
    None
}

fn drop_connection(
    transports: &mut [TcpTransportSlot; crate::consts::MAX_TCP_SOCKETS],
    sockets: &mut SocketSet<'_>,
    transport_index: usize,
    client_handle: rt::Handle,
) {
    let transport = &mut transports[transport_index];
    if transport.active {
        if let Some(socket_handle) = transport.socket_handle {
            sockets.get_mut::<tcp::Socket>(socket_handle).abort();
        }
        if transport.control_handle != rt::INVALID_HANDLE {
            let _ = rt::handle_close(transport.control_handle);
        }
        *transport = TcpTransportSlot::empty();
    }
    if client_handle != rt::INVALID_HANDLE {
        let _ = rt::handle_close(client_handle);
    }
}

pub(crate) fn close_listener(
    log_handle: rt::Handle,
    transports: &mut [TcpTransportSlot; crate::consts::MAX_TCP_SOCKETS],
    sockets: &mut SocketSet<'_>,
    slot: &mut TcpListenerSlot,
) {
    if let Some(socket_handle) = slot.socket_handle.take() {
        sockets.get_mut::<tcp::Socket>(socket_handle).abort();
    }
    while let Some((transport_index, client_handle)) = slot.pop_accept() {
        drop_connection(transports, sockets, transport_index, client_handle);
    }
    let _ = emit_log(
        log_handle,
        LogSeverity::Info,
        LogEvent::NetworkSocketClosed,
        0,
        slot.local_port as u64,
    );
    if slot.control_handle != rt::INVALID_HANDLE {
        let _ = rt::handle_close(slot.control_handle);
    }
    *slot = TcpListenerSlot::empty();
}

/// Drive inbound connections: adopt handshakes that landed on a listener's
/// pool handle into the matching (inactive) transport slot, queue them for
/// AcceptRequest, and re-arm pending listeners on freed pool handles.
pub(crate) fn pump_listeners(
    log_handle: rt::Handle,
    listeners: &mut [TcpListenerSlot; MAX_TCP_LISTENERS],
    transports: &mut [TcpTransportSlot; crate::consts::MAX_TCP_SOCKETS],
    tcp_handles: [SocketHandle; crate::consts::MAX_TCP_SOCKETS],
    sockets: &mut SocketSet<'_>,
    firewall: &mut FirewallState,
    iface_index: u16,
) -> rt::Result<()> {
    for listener_index in 0..listeners.len() {
        if !listeners[listener_index].active {
            continue;
        }

        // Pending re-arm: claim any freed pool handle and listen again.
        if listeners[listener_index].socket_handle.is_none() {
            let Some(handle) = free_pool_handle(listeners, transports, tcp_handles) else {
                continue;
            };
            let port = listeners[listener_index].local_port;
            let rearmed = {
                let socket = sockets.get_mut::<tcp::Socket>(handle);
                if socket.is_open() {
                    socket.abort();
                }
                socket.listen(port).is_ok()
            };
            if rearmed {
                listeners[listener_index].socket_handle = Some(handle);
            } else {
                continue;
            }
        }

        let Some(socket_handle) = listeners[listener_index].socket_handle else {
            continue;
        };
        let state = sockets.get_mut::<tcp::Socket>(socket_handle).state();
        if state == tcp::State::Listen || state == tcp::State::Closed {
            continue;
        }

        // Invariant: this pool handle belongs to an inactive transport slot.
        let Some(transport_index) = tcp_handles
            .iter()
            .position(|&handle| handle == socket_handle)
        else {
            continue;
        };
        if transports[transport_index].active {
            continue;
        }

        // Firewall gate: inspect the inbound peer before adopting it. A deny
        // aborts the handshake and re-arms the listener. IPv6 inbound
        // connections are outside the v0 slice (listeners stay v4); the
        // handshake is still aborted cleanly by treating them as filtered.
        let endpoints = {
            let socket = sockets.get_mut::<tcp::Socket>(socket_handle);
            match (socket.remote_endpoint(), socket.local_endpoint()) {
                (Some(remote), Some(local)) => match remote.addr {
                    smoltcp::wire::IpAddress::Ipv4(address) => {
                        Some((address, remote.port, local.port))
                    }
                    smoltcp::wire::IpAddress::Ipv6(_) => None,
                },
                _ => None,
            }
        };
        let Some((remote_address, remote_port, local_port)) = endpoints else {
            continue;
        };
        if !firewall.decide(
            Direction::Inbound,
            Proto::Tcp,
            local_port,
            remote_port,
            iface_index,
            RemoteAddress::V4(remote_address.octets()),
        ) {
            sockets.get_mut::<tcp::Socket>(socket_handle).abort();
            listeners[listener_index].socket_handle = None;
            let _ = rt::write_logf(
                "network",
                format_args!(
                    "firewall deny inbound tcp local={} remote={}:{}",
                    local_port,
                    ipv4_to_u32(remote_address),
                    remote_port
                ),
            );
            continue;
        }

        let session = rt::channel_create()?;
        let now = rt::monotonic_now().unwrap_or(0);
        transports[transport_index] = TcpTransportSlot {
            active: true,
            control_handle: session.first,
            socket_handle: Some(socket_handle),
            state: NetworkSocketState::Connecting,
            remote_address,
            remote_port,
            local_port,
            rx_bytes: 0,
            tx_bytes: 0,
            opened_at_ticks: now,
            last_activity_ticks: now,
        };
        // The connection keeps living on the adopted handle; go pending until
        // a pool handle frees up for listening again.
        listeners[listener_index].socket_handle = None;

        let backlog = listeners[listener_index].backlog;
        if listeners[listener_index].can_queue(backlog) {
            listeners[listener_index].push_accept(transport_index, session.second);
            let _ = emit_log(
                log_handle,
                LogSeverity::Info,
                LogEvent::NetworkSocketOpened,
                ipv4_to_u32(remote_address) as u64,
                remote_port as u64,
            );
        } else {
            // Backlog exhausted: refuse the connection instead of queueing it.
            drop_connection(transports, sockets, transport_index, session.second);
        }
    }
    Ok(())
}

pub(crate) fn handle_listener_request(
    log_handle: rt::Handle,
    listener_index: usize,
    listeners: &mut [TcpListenerSlot; MAX_TCP_LISTENERS],
    transports: &mut [TcpTransportSlot; crate::consts::MAX_TCP_SOCKETS],
    sockets: &mut SocketSet<'_>,
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
            reply.words[1] = listener_index as u64;
            reply.words[2] = NetworkSocketKind::TcpStream as u32 as u64;
            reply.words[3] = NetworkSocketState::Connecting as u32 as u64;
            reply.words[4] = 0;
            reply.words[5] = 0;
            reply.words[6] = listeners[listener_index].local_port as u64;
            reply.words[7] = listeners[listener_index].accept_len as u64;
            reply
        }
        x if x == NetworkSocketTag::AcceptRequest as u32 => {
            let mut reply = RawMessage::empty(NetworkSocketTag::AcceptReply as u32);
            reply.word_count = 3;
            match listeners[listener_index].pop_accept() {
                Some((transport_index, client_handle)) => {
                    let transport = &transports[transport_index];
                    reply.words[0] = NetworkStatus::Ok as u32 as u64;
                    reply.words[1] = ipv4_to_u32(transport.remote_address) as u64;
                    reply.words[2] = transport.remote_port as u64;
                    reply.handle_count = 1;
                    reply.handles[0] = client_handle;
                    reply.handle_rights[0] = rt::rights::SEND | rt::rights::RECEIVE;
                }
                None => {
                    // Nonblocking contract: empty accept queue -> Busy.
                    reply.words[0] = NetworkStatus::Busy as u32 as u64;
                    reply.words[1] = 0;
                    reply.words[2] = 0;
                }
            }
            reply
        }
        x if x == NetworkSocketTag::CloseRequest as u32 => {
            let mut listener = TcpListenerSlot::empty();
            core::mem::swap(&mut listener, &mut listeners[listener_index]);
            close_listener(log_handle, transports, sockets, &mut listener);
            // The control channel is gone; nothing to reply to.
            return Ok(());
        }
        _ => return Ok(()),
    };
    send_reply(reply_handle, reply);
    Ok(())
}
