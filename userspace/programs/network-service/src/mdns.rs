//! mDNS-LITE responder codec (honest subset of RFC 6762/6763).
//!
//! Supported: unicast UDP queries on port 5353 whose single question asks for
//! the A record (or ANY) of `<hostname>.local`, answered with one A record.
//! The answer owner name uses a compression pointer back to the question name
//! at offset 12 (the standard 0xC00C encoding).
//!
//! Not supported (and not claimed): multicast group semantics, probing /
//! conflict resolution, SRV/TXT/PTR records, DNS-SD service browsing, LLMNR.
//! Full mDNS remains open work; see docs/roadmap.md S7.
//!
//! Pure functions over byte slices so the codec is host-unit-testable.

use smoltcp::{
    iface::{Interface, SocketHandle, SocketSet},
    socket::udp,
    wire::Ipv4Address,
};

use serviceos_userspace_runtime as rt;

use crate::{
    consts::MDNS_TTL_SECONDS, device::KernelPacketDevice, dnsmsg::NameBuf, types::HostIdentity,
};

const RTYPE_A: u16 = 1;
const RTYPE_ANY: u16 = 255;
const QCLASS_IN: u16 = 1;
/// mDNS sets the cache-flush bit (0x8000) on answer classes; queries may also
/// carry the top bit (unicast-response request). Mask it before comparing.
const CLASS_TOP_BIT: u16 = 0x8000;

const FLAG_RESPONSE: u16 = 0x8000;
const FLAG_AUTHORITATIVE: u16 = 0x0400;

/// Parse an mDNS query and decide whether it asks for this host's A record.
/// Returns the transaction id when the question should be answered.
pub(crate) fn parse_local_a_query(message: &[u8], hostname: &[u8]) -> Option<u16> {
    if message.len() < 12 || hostname.is_empty() {
        return None;
    }
    let flags = u16::from_be_bytes([message[2], message[3]]);
    if flags & FLAG_RESPONSE != 0 {
        return None;
    }
    let question_count = u16::from_be_bytes([message[4], message[5]]);
    if question_count != 1 {
        return None;
    }
    let mut pos = 12usize;
    let name = NameBuf::decode(message, &mut pos)?;
    let text = name.as_str()?;
    if !matches_local_name(text, hostname) {
        return None;
    }
    if message.len() < pos + 4 {
        return None;
    }
    let qtype = u16::from_be_bytes([message[pos], message[pos + 1]]);
    let qclass = u16::from_be_bytes([message[pos + 2], message[pos + 3]]);
    if qclass & !CLASS_TOP_BIT != QCLASS_IN {
        return None;
    }
    if qtype != RTYPE_A && qtype != RTYPE_ANY {
        return None;
    }
    Some(u16::from_be_bytes([message[0], message[1]]))
}

/// Case-insensitive `<hostname>.local` comparison.
pub(crate) fn matches_local_name(name: &str, hostname: &[u8]) -> bool {
    let bytes = name.as_bytes();
    if bytes.len() != hostname.len() + ".local".len() {
        return false;
    }
    let (label, suffix) = bytes.split_at(hostname.len());
    label.eq_ignore_ascii_case(hostname) && suffix.eq_ignore_ascii_case(b".local")
}

/// Serialize the A-record response into `buffer`. Returns packet length.
/// Layout: header | uncompressed question name (`<hostname>.local`) + qtype +
/// qclass | answer whose owner name is the 0xC00C pointer to offset 12.
pub(crate) fn build_response(
    buffer: &mut [u8],
    id: u16,
    hostname: &[u8],
    address: Ipv4Address,
) -> Option<usize> {
    if hostname.is_empty() || hostname.len() > 63 {
        return None;
    }
    let question_name_len = 2 + hostname.len() + ".local".len(); // labels + root
    let total = 12 + question_name_len + 4 + (2 + 10 + 4);
    if buffer.len() < total {
        return None;
    }

    buffer[0..2].copy_from_slice(&id.to_be_bytes());
    buffer[2..4].copy_from_slice(&(FLAG_RESPONSE | FLAG_AUTHORITATIVE).to_be_bytes());
    buffer[4..6].copy_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    buffer[6..8].copy_from_slice(&1u16.to_be_bytes()); // ANCOUNT
    buffer[8..12].fill(0);

    let mut pos = 12usize;
    pos += write_label(&mut buffer[pos..], hostname);
    pos += write_label(&mut buffer[pos..], b"local");
    buffer[pos] = 0;
    pos += 1;
    buffer[pos..pos + 2].copy_from_slice(&RTYPE_A.to_be_bytes());
    buffer[pos + 2..pos + 4].copy_from_slice(&QCLASS_IN.to_be_bytes());
    pos += 4;

    // Answer: owner name = pointer to question name at offset 12.
    buffer[pos] = 0xC0;
    buffer[pos + 1] = 0x0C;
    buffer[pos + 2..pos + 4].copy_from_slice(&RTYPE_A.to_be_bytes());
    // Cache-flush class like conventional unique mDNS records.
    buffer[pos + 4..pos + 6].copy_from_slice(&(QCLASS_IN | CLASS_TOP_BIT).to_be_bytes());
    buffer[pos + 6..pos + 10].copy_from_slice(&MDNS_TTL_SECONDS.to_be_bytes());
    buffer[pos + 10..pos + 12].copy_from_slice(&4u16.to_be_bytes()); // RDLENGTH
    buffer[pos + 12..pos + 16].copy_from_slice(&address.octets());
    Some(pos + 16)
}

fn write_label(buffer: &mut [u8], label: &[u8]) -> usize {
    buffer[0] = label.len() as u8;
    buffer[1..1 + label.len()].copy_from_slice(label);
    1 + label.len()
}

/// Drain queued queries and answer A/ANY questions for `<hostname>.local`
/// with a unicast response to the querier. Called every main-loop iteration
/// once the interface has an address.
pub(crate) fn pump(
    iface: &mut Interface,
    device: &mut KernelPacketDevice,
    sockets: &mut SocketSet<'_>,
    mdns_handle: SocketHandle,
    identity: &HostIdentity,
    local_address: Ipv4Address,
) -> rt::Result<()> {
    let hostname = &identity.name[..identity.name_len];
    loop {
        let mut query = [0u8; 512];
        let (count, endpoint) = {
            let socket = sockets.get_mut::<udp::Socket>(mdns_handle);
            if !socket.can_recv() {
                break;
            }
            let (count, metadata) = socket.recv_slice(&mut query).map_err(|_| rt::Error::Busy)?;
            (count, metadata.endpoint)
        };
        let Some(id) = parse_local_a_query(&query[..count], hostname) else {
            continue;
        };
        let mut response = [0u8; 128];
        let Some(response_len) = build_response(&mut response, id, hostname, local_address) else {
            continue;
        };
        let _ = iface.poll(crate::util::now_instant(), device, sockets);
        let socket = sockets.get_mut::<udp::Socket>(mdns_handle);
        let _ = socket.send_slice(&response[..response_len], endpoint);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Standard query layout: header | labels | root | qtype | qclass.
    fn build_query(id: u16, name_labels: &[&[u8]], qtype: u16, qclass: u16) -> ([u8; 128], usize) {
        let mut buffer = [0u8; 128];
        buffer[0..2].copy_from_slice(&id.to_be_bytes());
        buffer[4..6].copy_from_slice(&1u16.to_be_bytes());
        let mut pos = 12usize;
        for label in name_labels {
            pos += write_label(&mut buffer[pos..], label);
        }
        buffer[pos] = 0;
        pos += 1;
        buffer[pos..pos + 2].copy_from_slice(&qtype.to_be_bytes());
        buffer[pos + 2..pos + 4].copy_from_slice(&qclass.to_be_bytes());
        (buffer, pos + 4)
    }

    #[test]
    fn answers_a_query_with_compressed_owner_name() {
        let hostname = b"serviceos";
        let (query, len) = build_query(0x1234, &[hostname, b"local"], RTYPE_A, QCLASS_IN);
        let mut response = [0u8; 128];
        let response_len = build_response(
            &mut response,
            0x1234,
            hostname,
            Ipv4Address::new(10, 0, 2, 15),
        )
        .expect("response fits");
        assert_eq!(parse_local_a_query(&query[..len], hostname), Some(0x1234));

        // Response flag + authoritative, one question, one answer.
        let flags = u16::from_be_bytes([response[2], response[3]]);
        assert_eq!(flags & FLAG_RESPONSE, FLAG_RESPONSE);
        assert_eq!(u16::from_be_bytes([response[6], response[7]]), 1);

        // Answer owner name is a compression pointer to offset 12.
        let answer_offset = 12 + (2 + hostname.len() + 6) + 4;
        assert_eq!(response[answer_offset], 0xC0);
        assert_eq!(response[answer_offset + 1], 0x0C);

        // Decoding the owner name through the shared parser yields the FQDN.
        let mut cursor = answer_offset;
        let decoded =
            NameBuf::decode(&response[..response_len], &mut cursor).expect("owner name decodes");
        assert_eq!(decoded.as_str(), Some("serviceos.local"));

        // RDATA carries the address.
        let rdlength =
            u16::from_be_bytes([response[answer_offset + 10], response[answer_offset + 11]]);
        assert_eq!(rdlength, 4);
        assert_eq!(
            &response[answer_offset + 12..answer_offset + 16],
            &[10, 0, 2, 15]
        );
        assert_eq!(response_len, answer_offset + 16);
    }

    #[test]
    fn matching_is_case_insensitive_and_requires_local_suffix() {
        assert!(matches_local_name("ServiceOS.local", b"serviceos"));
        assert!(matches_local_name("serviceos.LOCAL", b"serviceos"));
        assert!(!matches_local_name("other.local", b"serviceos"));
        assert!(!matches_local_name("serviceos.example.com", b"serviceos"));
        assert!(!matches_local_name("serviceoslocal", b"serviceos"));
    }

    #[test]
    fn rejects_responses_other_names_classes_and_types() {
        let hostname = b"serviceos";
        let (query, len) = build_query(1, &[b"otherhost", b"local"], RTYPE_A, QCLASS_IN);
        assert_eq!(parse_local_a_query(&query[..len], hostname), None);

        let (query, len) = build_query(1, &[hostname, b"local"], 28, QCLASS_IN);
        assert_eq!(parse_local_a_query(&query[..len], hostname), None); // AAAA

        let (query, len) = build_query(1, &[hostname, b"local"], RTYPE_A, 3); // CHaos
        assert_eq!(parse_local_a_query(&query[..len], hostname), None);

        // A response (QR set) is never answered.
        let (mut query, len) = build_query(1, &[hostname, b"local"], RTYPE_A, QCLASS_IN);
        query[2] = 0x80;
        assert_eq!(parse_local_a_query(&query[..len], hostname), None);

        assert_eq!(parse_local_a_query(&query[..8], hostname), None);
    }

    #[test]
    fn any_query_type_is_answered_and_cache_flush_class_accepted() {
        let hostname = b"os";
        let (query, len) = build_query(
            7,
            &[hostname, b"local"],
            RTYPE_ANY,
            QCLASS_IN | CLASS_TOP_BIT,
        );
        assert_eq!(parse_local_a_query(&query[..len], hostname), Some(7));
    }
}
