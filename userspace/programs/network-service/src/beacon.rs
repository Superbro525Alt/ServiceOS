//! Discovery beacon wire codec (service-local UDP protocol, not a standard).
//!
//! Frame layout (big-endian):
//! ```text
//! offset  size  field
//! 0       1     magic 'S'
//! 1       1     version (1)
//! 2       1     flags: bit0 ANNOUNCE, bit1 QUERY
//! 3       1     sender name length N (0 for pure queries)
//! 4       N     sender name bytes (ASCII)
//! 4+N     4     sender IPv4 address (0 on queries)
//! 8+N     2     service count C
//! 10+N    ...   C entries: [u8 name length][name bytes][u16 port]
//! ```
//! Pure functions over byte slices so the codec is host-unit-testable.

use smoltcp::{
    iface::{Interface, SocketHandle, SocketSet},
    socket::udp,
    wire::{IpEndpoint, IpAddress, Ipv4Address},
};

use serviceos_userspace_runtime as rt;

use crate::{
    consts::{
        BEACON_FLAG_ANNOUNCE, BEACON_FLAG_QUERY, BEACON_UDP_PORT, BEACON_VERSION,
        MAX_BEACON_NAME_BYTES, MAX_LOCAL_SERVICES, MAX_SERVICE_NAME_BYTES,
    },
    device::KernelPacketDevice,
    discover::{PeerTable, Registry},
    types::HostIdentity,
};

const MAGIC: u8 = b'S';

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BeaconService {
    pub(crate) name_len: usize,
    pub(crate) name: [u8; MAX_SERVICE_NAME_BYTES],
    pub(crate) port: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BeaconFrame {
    pub(crate) flags: u8,
    pub(crate) name_len: usize,
    pub(crate) name: [u8; MAX_BEACON_NAME_BYTES],
    pub(crate) address: [u8; 4],
    pub(crate) service_count: usize,
    pub(crate) services: [BeaconService; MAX_LOCAL_SERVICES],
}

impl BeaconFrame {
    const fn empty() -> Self {
        Self {
            flags: 0,
            name_len: 0,
            name: [0; MAX_BEACON_NAME_BYTES],
            address: [0; 4],
            service_count: 0,
            services: [BeaconService {
                name_len: 0,
                name: [0; MAX_SERVICE_NAME_BYTES],
                port: 0,
            }; MAX_LOCAL_SERVICES],
        }
    }
}

/// Serialize an announce frame advertising `sender_name` + its services.
pub(crate) fn encode(
    buffer: &mut [u8],
    flags: u8,
    sender_name: &[u8],
    address: [u8; 4],
    services: &[BeaconService],
) -> Option<usize> {
    if buffer.len() < 10 || sender_name.len() > MAX_BEACON_NAME_BYTES {
        return None;
    }
    if services.len() > MAX_LOCAL_SERVICES {
        return None;
    }
    let mut pos = 0usize;
    buffer[pos] = MAGIC;
    buffer[pos + 1] = BEACON_VERSION;
    buffer[pos + 2] = flags;
    buffer[pos + 3] = sender_name.len() as u8;
    pos += 4;
    buffer[pos..pos + sender_name.len()].copy_from_slice(sender_name);
    pos += sender_name.len();
    buffer[pos..pos + 4].copy_from_slice(&address);
    pos += 4;
    buffer[pos..pos + 2].copy_from_slice(&(services.len() as u16).to_be_bytes());
    pos += 2;
    for service in services {
        if service.name_len > service.name.len()
            || service.name_len > MAX_SERVICE_NAME_BYTES
            || buffer.len() < pos + 3 + service.name_len
        {
            return None;
        }
        buffer[pos] = service.name_len as u8;
        buffer[pos + 1..pos + 1 + service.name_len]
            .copy_from_slice(&service.name[..service.name_len]);
        buffer[pos + 1 + service.name_len..pos + 3 + service.name_len]
            .copy_from_slice(&service.port.to_be_bytes());
        pos += 3 + service.name_len;
    }
    Some(pos)
}

pub(crate) fn encode_announce(
    buffer: &mut [u8],
    sender_name: &[u8],
    address: [u8; 4],
    services: &[BeaconService],
) -> Option<usize> {
    encode(buffer, BEACON_FLAG_ANNOUNCE, sender_name, address, services)
}

pub(crate) fn encode_query(buffer: &mut [u8]) -> Option<usize> {
    encode(buffer, BEACON_FLAG_QUERY, &[], [0; 4], &[])
}

/// Bounded parse; rejects bad magic/version, truncated fields, oversized
/// names/counts.
pub(crate) fn decode(buffer: &[u8]) -> Option<BeaconFrame> {
    if buffer.len() < 10 || buffer[0] != MAGIC || buffer[1] != BEACON_VERSION {
        return None;
    }
    let mut frame = BeaconFrame::empty();
    frame.flags = buffer[2];
    if frame.flags & !(BEACON_FLAG_ANNOUNCE | BEACON_FLAG_QUERY) != 0 {
        return None;
    }
    let name_len = buffer[3] as usize;
    if name_len > MAX_BEACON_NAME_BYTES || buffer.len() < 4 + name_len + 6 {
        return None;
    }
    frame.name_len = name_len;
    frame.name[..name_len].copy_from_slice(&buffer[4..4 + name_len]);
    let mut pos = 4 + name_len;
    frame.address.copy_from_slice(&buffer[pos..pos + 4]);
    pos += 4;
    let service_count = u16::from_be_bytes([buffer[pos], buffer[pos + 1]]) as usize;
    pos += 2;
    if service_count > MAX_LOCAL_SERVICES {
        return None;
    }
    frame.service_count = service_count;
    for index in 0..service_count {
        if pos >= buffer.len() {
            return None;
        }
        let entry_name_len = buffer[pos] as usize;
        if entry_name_len > MAX_SERVICE_NAME_BYTES || buffer.len() < pos + 3 + entry_name_len {
            return None;
        }
        let service = &mut frame.services[index];
        service.name_len = entry_name_len;
        service.name[..entry_name_len].copy_from_slice(&buffer[pos + 1..pos + 1 + entry_name_len]);
        service.port = u16::from_be_bytes([
            buffer[pos + 1 + entry_name_len],
            buffer[pos + 2 + entry_name_len],
        ]);
        pos += 3 + entry_name_len;
    }
    Some(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(name: &[u8], port: u16) -> BeaconService {
        let mut entry = BeaconService {
            name_len: name.len(),
            name: [0; MAX_SERVICE_NAME_BYTES],
            port,
        };
        entry.name[..name.len()].copy_from_slice(name);
        entry
    }

    #[test]
    fn announce_round_trips() {
        let services = [service(b"shell", 4021), service(b"logs", 4022)];
        let mut buffer = [0u8; 256];
        let len = encode_announce(&mut buffer, b"node7", [192, 168, 7, 9], &services)
            .expect("announce encodes");
        let frame = decode(&buffer[..len]).expect("decodes");
        assert_eq!(frame.flags, BEACON_FLAG_ANNOUNCE);
        assert_eq!(&frame.name[..frame.name_len], b"node7");
        assert_eq!(frame.address, [192, 168, 7, 9]);
        assert_eq!(frame.service_count, 2);
        assert_eq!(frame.services[0].port, 4021);
        assert_eq!(&frame.services[0].name[..5], b"shell");
        assert_eq!(frame.services[1].port, 4022);
        // Encoded size is header(4) + name(5) + ip(4) + count(2) +
        // (3+5) + (3+4).
        assert_eq!(len, 30);
    }

    #[test]
    fn query_round_trips_with_no_services() {
        let mut buffer = [0u8; 64];
        let len = encode_query(&mut buffer).expect("query encodes");
        let frame = decode(&buffer[..len]).expect("decodes");
        assert_eq!(frame.flags, BEACON_FLAG_QUERY);
        assert_eq!(frame.name_len, 0);
        assert_eq!(frame.service_count, 0);
        assert_eq!(frame.address, [0; 4]);
        assert_eq!(len, 10);
    }

    #[test]
    fn rejects_malformed_frames() {
        let mut buffer = [0u8; 64];
        let len = encode_query(&mut buffer).unwrap();

        let mut bad_magic = buffer;
        bad_magic[0] = b'X';
        assert_eq!(decode(&bad_magic[..len]), None);

        let mut bad_version = buffer;
        bad_version[1] = 9;
        assert_eq!(decode(&bad_version[..len]), None);

        let mut bad_flags = buffer;
        bad_flags[2] = 0xF0;
        assert_eq!(decode(&bad_flags[..len]), None);

        assert_eq!(decode(&buffer[..len - 1]), None); // truncated
        assert_eq!(decode(&[]), None);
    }

    #[test]
    fn rejects_oversized_service_entries_and_counts() {
        let mut services = [service(b"a", 1); MAX_LOCAL_SERVICES + 1];
        let _ = &mut services;
        let mut buffer = [0u8; 512];
        assert_eq!(
            encode_announce(&mut buffer, b"n", [1, 2, 3, 4], &services),
            None
        );

        let mut evil = [0u8; 64];
        evil[0] = MAGIC;
        evil[1] = BEACON_VERSION;
        evil[2] = BEACON_FLAG_ANNOUNCE;
        evil[3] = 0;
        evil[4..8].copy_from_slice(&[1, 2, 3, 4]);
        evil[8..10].copy_from_slice(&(MAX_LOCAL_SERVICES as u16 + 1).to_be_bytes());
        assert_eq!(decode(&evil), None);
    }
}

/// Convert registry entries into beacon service records.
fn registry_records(registry: &Registry) -> ([BeaconService; MAX_LOCAL_SERVICES], usize) {
    let mut services = [BeaconService {
        name_len: 0,
        name: [0; MAX_SERVICE_NAME_BYTES],
        port: 0,
    }; MAX_LOCAL_SERVICES];
    for (index, entry) in registry.snapshot().iter().enumerate() {
        services[index] = BeaconService {
            name_len: entry.name_len,
            name: entry.name,
            port: entry.port,
        };
    }
    (services, registry.count)
}

fn sender_name(identity: &HostIdentity) -> &[u8] {
    &identity.name[..identity.name_len.min(MAX_BEACON_NAME_BYTES)]
}

/// Broadcast one announce frame. Returns Ok(false) when the frame could not
/// be emitted yet (no tx buffer); callers just retry next loop.
pub(crate) fn announce(
    iface: &mut Interface,
    device: &mut KernelPacketDevice,
    sockets: &mut SocketSet<'_>,
    beacon_handle: SocketHandle,
    registry: &Registry,
    identity: &HostIdentity,
    local_address: Ipv4Address,
) -> rt::Result<bool> {
    let (services, count) = registry_records(registry);
    let mut frame = [0u8; 256];
    let Some(len) = encode_announce(
        &mut frame,
        sender_name(identity),
        local_address.octets(),
        &services[..count],
    ) else {
        return Ok(false);
    };
    let target = IpEndpoint::new(
        IpAddress::Ipv4(Ipv4Address::BROADCAST),
        BEACON_UDP_PORT,
    );
    let socket = sockets.get_mut::<udp::Socket>(beacon_handle);
    let queued = socket.send_slice(&frame[..len], target).is_ok();
    // Flush the queued datagram immediately.
    let _ = iface.poll(crate::util::now_instant(), device, sockets);
    Ok(queued)
}

/// Broadcast one QUERY solicitation so listening peers announce back.
pub(crate) fn solicit(
    iface: &mut Interface,
    device: &mut KernelPacketDevice,
    sockets: &mut SocketSet<'_>,
    beacon_handle: SocketHandle,
) -> rt::Result<bool> {
    let mut frame = [0u8; 64];
    let Some(len) = encode_query(&mut frame) else {
        return Ok(false);
    };
    let target = IpEndpoint::new(IpAddress::Ipv4(Ipv4Address::BROADCAST), BEACON_UDP_PORT);
    let socket = sockets.get_mut::<udp::Socket>(beacon_handle);
    let queued = socket.send_slice(&frame[..len], target).is_ok();
    let _ = iface.poll(crate::util::now_instant(), device, sockets);
    Ok(queued)
}

/// Drain received beacons into the peer table and answer QUERY frames with a
/// unicast announce. Returns the current peer count.
pub(crate) fn pump(
    iface: &mut Interface,
    device: &mut KernelPacketDevice,
    sockets: &mut SocketSet<'_>,
    beacon_handle: SocketHandle,
    registry: &Registry,
    peers: &mut PeerTable,
    identity: &HostIdentity,
    now_ms: u64,
    local_address: Ipv4Address,
) -> rt::Result<usize> {
    loop {
        let mut buffer = [0u8; 512];
        let (count, endpoint) = {
            let socket = sockets.get_mut::<udp::Socket>(beacon_handle);
            if !socket.can_recv() {
                break;
            }
            let (count, metadata) =
                socket.recv_slice(&mut buffer).map_err(|_| rt::Error::Busy)?;
            (count, metadata.endpoint)
        };
        let Some(frame) = decode(&buffer[..count]) else {
            continue;
        };
        if frame.name_len == 0 {
            continue;
        }
        // Never record ourselves (loopback echoes of our own announces).
        if frame.name_len == identity.name_len
            && frame.name[..frame.name_len] == identity.name[..identity.name_len]
        {
            continue;
        }
        peers.note(&frame.name[..frame.name_len], frame.address, now_ms)?;

        if frame.flags & BEACON_FLAG_QUERY != 0 {
            let (services, service_count) = registry_records(registry);
            let mut reply = [0u8; 256];
            if let Some(len) = encode_announce(
                &mut reply,
                sender_name(identity),
                local_address.octets(),
                &services[..service_count],
            ) {
                // Unicast announce back to the querier.
                let _ = iface.poll(crate::util::now_instant(), device, sockets);
                let socket = sockets.get_mut::<udp::Socket>(beacon_handle);
                let _ = socket.send_slice(&reply[..len], endpoint);
            }
        }
    }
    Ok(peers.count)
}
