use crate::{
    channel_create, channel_receive_blocking, channel_send, handle_close, network_status_error,
    network_status_from_word, pack_bytes, packet_backend_from_word, packet_link_state_from_word,
    rights, unpack_mac, Error, Handle, NetworkInterfaceStatusInfo, NetworkStatus, NetworkTag,
    RawMessage, Result, IPC_MAX_WORDS,
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
    if response.tag != NetworkTag::InterfaceStatusReply as u32 || response.word_count < 12 {
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
        address: response.words[5] as u32,
        prefix_len: response.words[6] as u8,
        gateway: response.words[7] as u32,
        mac: unpack_mac(response.words[8]),
        rx_packets: response.words[9],
        tx_packets: response.words[10],
        dropped_packets: response.words[11],
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
