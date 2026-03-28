use crate::{
    channel_create, channel_receive_blocking, channel_send, handle_close, network_config_mode_from_word,
    network_config_state_from_word, network_socket_kind_from_word, network_socket_state_from_word,
    network_status_error, network_status_from_word, pack_bytes, packet_backend_from_word,
    packet_link_state_from_word, rights, unpack_bytes, unpack_mac, Error, Handle,
    NetworkInterfaceStatusInfo, NetworkSocketInfo, NetworkSocketKind, NetworkSocketTag,
    NetworkStatus, NetworkTag, RawMessage, Result, IPC_MAX_WORDS,
};

pub fn network_interface_count(network_handle: Handle) -> Result<usize> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(NetworkTag::InterfaceListRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(network_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != NetworkTag::InterfaceListReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    match network_status_from_word(response.words[0]) {
        NetworkStatus::Ok => Ok(response.words[1] as usize),
        NetworkStatus::Busy => Err(Error::Busy),
        NetworkStatus::Unsupported => Err(Error::Unsupported),
        _ => Err(Error::InvalidArgument),
    }
}

pub fn network_interface_status(
    network_handle: Handle,
    index: usize,
) -> Result<Option<NetworkInterfaceStatusInfo>> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(NetworkTag::InterfaceStatusRequest as u32);
    request.word_count = 1;
    request.words[0] = index as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(network_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != NetworkTag::InterfaceStatusReply as u32 || response.word_count < 15 {
        return Err(Error::InvalidArgument);
    }

    let status = network_status_from_word(response.words[0]);
    if status == NetworkStatus::NotFound {
        return Ok(None);
    }
    if status != NetworkStatus::Ok {
        return Err(network_status_error(status));
    }

    Ok(Some(NetworkInterfaceStatusInfo {
        index: response.words[1] as u32,
        backend: packet_backend_from_word(response.words[2]),
        link_state: packet_link_state_from_word(response.words[3]),
        mtu: response.words[4] as u32,
        config_mode: network_config_mode_from_word(response.words[5]),
        config_state: network_config_state_from_word(response.words[6]),
        address: response.words[7] as u32,
        prefix_len: response.words[8] as u8,
        gateway: response.words[9] as u32,
        dns_server: response.words[10] as u32,
        mac: unpack_mac(response.words[11]),
        rx_packets: response.words[12],
        tx_packets: response.words[13],
        dropped_packets: response.words[14],
    }))
}

pub fn network_resolve(
    network_handle: Handle,
    name: &str,
    addresses: &mut [u32],
) -> Result<usize> {
    let name_bytes = name.as_bytes();
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(1)) * 8;
    if name_bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let reply = channel_create()?;
    let mut request = RawMessage::empty(NetworkTag::ResolveRequest as u32);
    request.word_count = 1 + pack_bytes(name_bytes, &mut request.words[1..])?;
    request.words[0] = name_bytes.len() as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(network_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != NetworkTag::ResolveReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }

    let status = network_status_from_word(response.words[0]);
    if status != NetworkStatus::Ok {
        return Err(network_status_error(status));
    }

    let count = response.words[1] as usize;
    if count > addresses.len() || (response.word_count as usize) < 2 + count {
        return Err(Error::BufferTooSmall);
    }
    for (index, address) in addresses.iter_mut().enumerate().take(count) {
        *address = response.words[2 + index] as u32;
    }
    Ok(count)
}

pub fn network_ping(network_handle: Handle, target: &str) -> Result<(u32, u64)> {
    let target_bytes = target.as_bytes();
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(1)) * 8;
    if target_bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let reply = channel_create()?;
    let mut request = RawMessage::empty(NetworkTag::PingRequest as u32);
    request.word_count = 1 + pack_bytes(target_bytes, &mut request.words[1..])?;
    request.words[0] = target_bytes.len() as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(network_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != NetworkTag::PingReply as u32 || response.word_count < 3 {
        return Err(Error::InvalidArgument);
    }

    let status = network_status_from_word(response.words[0]);
    if status != NetworkStatus::Ok {
        return Err(network_status_error(status));
    }

    Ok((response.words[1] as u32, response.words[2]))
}

pub fn network_socket_open(
    network_handle: Handle,
    kind: NetworkSocketKind,
    target: &str,
    port: u16,
) -> Result<Handle> {
    let target_bytes = target.as_bytes();
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(2)) * 8;
    if target_bytes.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let reply = channel_create()?;
    let mut request = RawMessage::empty(NetworkTag::SocketOpenRequest as u32);
    request.word_count = 2 + pack_bytes(target_bytes, &mut request.words[2..])?;
    request.words[0] = kind as u32 as u64;
    request.words[1] = ((target_bytes.len() as u64) << 16) | port as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(network_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != NetworkTag::SocketOpenReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }

    let status = network_status_from_word(response.words[0]);
    if status != NetworkStatus::Ok {
        return Err(network_status_error(status));
    }
    if response.handle_count < 1 {
        return Err(Error::InvalidArgument);
    }
    Ok(response.handles[0])
}

pub fn network_socket_list(
    network_handle: Handle,
    sockets: &mut [NetworkSocketInfo],
) -> Result<usize> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(NetworkTag::SocketListRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(network_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != NetworkTag::SocketListReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }

    let status = network_status_from_word(response.words[0]);
    if status != NetworkStatus::Ok {
        return Err(network_status_error(status));
    }

    let count = response.words[1] as usize;
    if count > sockets.len() || response.word_count as usize != 2 + count * 7 {
        return Err(Error::BufferTooSmall);
    }
    for (index, socket) in sockets.iter_mut().enumerate().take(count) {
        let base = 2 + index * 7;
        *socket = NetworkSocketInfo {
            slot: response.words[base] as u32,
            kind: network_socket_kind_from_word(response.words[base + 1]),
            state: network_socket_state_from_word(response.words[base + 2]),
            remote_address: response.words[base + 3] as u32,
            remote_port: response.words[base + 4] as u16,
            local_port: response.words[base + 5] as u16,
            rx_bytes: response.words[base + 6] >> 32,
            tx_bytes: response.words[base + 6] & 0xffff_ffff,
        };
    }
    Ok(count)
}

pub fn network_socket_status(socket_handle: Handle) -> Result<NetworkSocketInfo> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(NetworkSocketTag::StatusRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(socket_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != NetworkSocketTag::StatusReply as u32 || response.word_count < 8 {
        return Err(Error::InvalidArgument);
    }
    let status = network_status_from_word(response.words[0]);
    if status != NetworkStatus::Ok {
        return Err(network_status_error(status));
    }
    Ok(NetworkSocketInfo {
        slot: response.words[1] as u32,
        kind: network_socket_kind_from_word(response.words[2]),
        state: network_socket_state_from_word(response.words[3]),
        remote_address: response.words[4] as u32,
        remote_port: response.words[5] as u16,
        local_port: response.words[6] as u16,
        rx_bytes: response.words[7] >> 32,
        tx_bytes: response.words[7] & 0xffff_ffff,
    })
}

pub fn network_socket_send(socket_handle: Handle, payload: &[u8]) -> Result<usize> {
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(1)) * 8;
    if payload.len() > max_inline_bytes {
        return Err(Error::BufferTooSmall);
    }

    let reply = channel_create()?;
    let mut request = RawMessage::empty(NetworkSocketTag::SendRequest as u32);
    request.word_count = 1 + pack_bytes(payload, &mut request.words[1..])?;
    request.words[0] = payload.len() as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(socket_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != NetworkSocketTag::SendReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    let status = network_status_from_word(response.words[0]);
    if status != NetworkStatus::Ok {
        return Err(network_status_error(status));
    }
    Ok(response.words[1] as usize)
}

pub fn network_socket_receive(socket_handle: Handle, buffer: &mut [u8]) -> Result<usize> {
    let max_inline_bytes = (IPC_MAX_WORDS.saturating_sub(2)) * 8;
    let requested = buffer.len().min(max_inline_bytes);
    let reply = channel_create()?;
    let mut request = RawMessage::empty(NetworkSocketTag::ReceiveRequest as u32);
    request.word_count = 1;
    request.words[0] = requested as u64;
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(socket_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != NetworkSocketTag::ReceiveReply as u32 || response.word_count < 2 {
        return Err(Error::InvalidArgument);
    }
    let status = network_status_from_word(response.words[0]);
    if status != NetworkStatus::Ok {
        return Err(network_status_error(status));
    }
    let count = response.words[1] as usize;
    unpack_bytes(&response.words[2..response.word_count as usize], count, buffer)?;
    Ok(count)
}

pub fn network_socket_close(socket_handle: Handle) -> Result<()> {
    let reply = channel_create()?;
    let mut request = RawMessage::empty(NetworkSocketTag::CloseRequest as u32);
    request.handle_count = 1;
    request.handles[0] = reply.second;
    request.handle_rights[0] = rights::SEND;
    channel_send(socket_handle, &request)?;
    let _ = handle_close(reply.second);

    let mut response = RawMessage::empty(0);
    channel_receive_blocking(reply.first, &mut response)?;
    let _ = handle_close(reply.first);
    if response.tag != NetworkSocketTag::CloseReply as u32 || response.word_count < 1 {
        return Err(Error::InvalidArgument);
    }
    let status = network_status_from_word(response.words[0]);
    if status != NetworkStatus::Ok && status != NetworkStatus::Closed {
        return Err(network_status_error(status));
    }
    Ok(())
}
