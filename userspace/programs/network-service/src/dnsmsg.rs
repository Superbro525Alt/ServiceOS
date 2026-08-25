//! Minimal DNS wire codec: builds standard recursive queries and parses
//! responses for A / AAAA / CNAME / TXT records with name compression.
//! Pure functions over byte slices so the codec is host-unit-testable.

use crate::consts::{MAX_RESOLVER_NAME_BYTES, MAX_TXT_BYTES};

pub(crate) const RTYPE_A: u16 = 1;
pub(crate) const RTYPE_CNAME: u16 = 5;
pub(crate) const RTYPE_TXT: u16 = 16;
pub(crate) const RTYPE_AAAA: u16 = 28;

pub(crate) const RCODE_OK: u8 = 0;
pub(crate) const RCODE_SERVFAIL: u8 = 2;
pub(crate) const RCODE_NXDOMAIN: u8 = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum QueryType {
    A,
    Aaaa,
    Txt,
}

impl QueryType {
    pub(crate) fn rtype(self) -> u16 {
        match self {
            QueryType::A => RTYPE_A,
            QueryType::Aaaa => RTYPE_AAAA,
            QueryType::Txt => RTYPE_TXT,
        }
    }
}

/// Bounded fixed-size domain name.
#[derive(Clone, Copy, Debug)]
pub(crate) struct NameBuf {
    pub(crate) bytes: [u8; MAX_RESOLVER_NAME_BYTES],
    pub(crate) len: usize,
}

impl NameBuf {
    pub(crate) const fn empty() -> Self {
        Self {
            bytes: [0; MAX_RESOLVER_NAME_BYTES],
            len: 0,
        }
    }

    /// Build from a dotted ASCII name (no trailing dot). The resulting buffer
    /// holds dotted text (not wire labels) so as_str()/matches() are uniform
    /// across parsed requests and decoded responses. Returns None when the
    /// name is empty, exceeds the buffer, or carries an oversized label.
    pub(crate) fn parse(name: &str) -> Option<Self> {
        let mut out = Self::empty();
        if name.is_empty() || name.len() > out.bytes.len() {
            return None;
        }
        let mut wire_len = 0usize;
        for label in name.split('.') {
            if label.is_empty() || label.len() > 63 {
                return None;
            }
            wire_len += 1 + label.len();
        }
        if wire_len + 1 > out.bytes.len() {
            return None;
        }
        for label in name.split('.') {
            let label_len = label.len();
            out.bytes[out.len] = label_len as u8;
            out.len += 1;
            out.bytes[out.len..out.len + label_len].copy_from_slice(label.as_bytes());
            out.len += label_len;
        }
        out.to_dotted()
    }

    /// Decode a (possibly compressed) DNS name starting at `pos` within a full
    /// message. Bounds- and loop-guarded.
    pub(crate) fn decode(message: &[u8], pos: &mut usize) -> Option<Self> {
        let mut out = Self::empty();
        let mut cursor = *pos;
        let mut jumps = 0usize;
        let mut jumped = false;
        let mut next_after_jump = 0usize;
        loop {
            if cursor >= message.len() || out.len >= out.bytes.len() {
                return None;
            }
            let length = message[cursor];
            match length & 0xC0 {
                0x00 => {
                    cursor += 1;
                    if length == 0 {
                        if jumped {
                            *pos = next_after_jump;
                        } else {
                            *pos = cursor;
                        }
                        if out.len == 0 {
                            return Some(out); // root name
                        }
                        // Convert wire labels to dotted text form for uniform
                        // storage/comparison with request names.
                        return out.to_dotted();
                    }
                    if cursor + length as usize > message.len()
                        || out.len + 1 + length as usize > out.bytes.len()
                    {
                        return None;
                    }
                    out.bytes[out.len] = length;
                    out.len += 1;
                    out.bytes[out.len..out.len + length as usize]
                        .copy_from_slice(&message[cursor..cursor + length as usize]);
                    out.len += length as usize;
                    cursor += length as usize;
                }
                0xC0 => {
                    if cursor + 1 >= message.len() || jumps >= 8 {
                        return None;
                    }
                    let offset = (((length & 0x3F) as usize) << 8) | message[cursor + 1] as usize;
                    if offset >= message.len() {
                        return None;
                    }
                    if !jumped {
                        next_after_jump = cursor + 2;
                        jumped = true;
                    }
                    cursor = offset;
                    jumps += 1;
                }
                _ => return None,
            }
        }
    }

    fn to_dotted(mut self) -> Option<Self> {
        let mut dotted = [0u8; MAX_RESOLVER_NAME_BYTES];
        let mut out_len = 0usize;
        let mut cursor = 0usize;
        while cursor < self.len {
            let label_len = self.bytes[cursor] as usize;
            cursor += 1;
            if out_len != 0 {
                if out_len + 1 > dotted.len() {
                    return None;
                }
                dotted[out_len] = b'.';
                out_len += 1;
            }
            if out_len + label_len > dotted.len() {
                return None;
            }
            dotted[out_len..out_len + label_len]
                .copy_from_slice(&self.bytes[cursor..cursor + label_len]);
            out_len += label_len;
            cursor += label_len;
        }
        self.bytes = dotted;
        self.len = out_len;
        Some(self)
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        core::str::from_utf8(&self.bytes[..self.len]).ok()
    }

    pub(crate) fn matches(&self, name: &str) -> bool {
        self.len == name.len() && &self.bytes[..self.len] == name.as_bytes()
    }
}

/// Parsed answer section, capped to what fits in cache entries and inline IPC.
#[derive(Clone, Copy)]
pub(crate) struct DnsRecords {
    pub(crate) rcode: u8,
    pub(crate) qtype_matched: QueryType,
    pub(crate) a: [u32; crate::consts::MAX_CACHED_A_RECORDS],
    pub(crate) a_count: usize,
    pub(crate) aaaa: [[u8; 16]; 2],
    pub(crate) aaaa_count: usize,
    /// Records matching the queried type (A/AAAA/TXT) use these fields:
    pub(crate) cname: Option<NameBuf>,
    /// When the answer section carried a CNAME chain, the owner the final
    /// A/AAAA/TXT records belong to (the chain end). None = answers are keyed
    /// directly to the queried name.
    pub(crate) resolved_owner: Option<NameBuf>,
    pub(crate) txt: [u8; MAX_TXT_BYTES],
    pub(crate) txt_len: usize,
    /// Smallest TTL seen among records relevant to the query, milliseconds.
    pub(crate) min_ttl_ms: u64,
}

impl DnsRecords {
    fn new(qtype: QueryType) -> Self {
        Self {
            rcode: RCODE_OK,
            qtype_matched: qtype,
            a: [0; crate::consts::MAX_CACHED_A_RECORDS],
            a_count: 0,
            aaaa: [[0; 16]; 2],
            aaaa_count: 0,
            cname: None,
            resolved_owner: None,
            txt: [0; MAX_TXT_BYTES],
            txt_len: 0,
            min_ttl_ms: 0,
        }
    }
}

/// Serialize one recursive query into `buffer`. Returns packet length.
pub(crate) fn build_query(
    buffer: &mut [u8],
    id: u16,
    name: &str,
    qtype: QueryType,
) -> Option<usize> {
    // Validate through the same parser the resolver uses (length/label rules).
    let encoded = NameBuf::parse(name)?;
    if encoded.len != name.len() {
        return None;
    }
    let mut wire_len = 1usize; // root byte
    for label in name.split('.') {
        wire_len += 1 + label.len();
    }
    if buffer.len() < 12 + wire_len + 4 {
        return None;
    }
    buffer[0..2].copy_from_slice(&id.to_be_bytes());
    // RD=1, rest zero.
    buffer[2] = 0x01;
    buffer[3] = 0x00;
    buffer[4..6].copy_from_slice(&1u16.to_be_bytes());
    buffer[6..12].fill(0);
    let mut pos = 12;
    for label in name.split('.') {
        buffer[pos] = label.len() as u8;
        pos += 1;
        buffer[pos..pos + label.len()].copy_from_slice(label.as_bytes());
        pos += label.len();
    }
    buffer[pos] = 0;
    pos += 1;
    buffer[pos..pos + 2].copy_from_slice(&qtype.rtype().to_be_bytes());
    buffer[pos + 2..pos + 4].copy_from_slice(&1u16.to_be_bytes()); // IN
    Some(pos + 4)
}

/// Parse a response previously produced for `expected_id`. Returns None when
/// the packet is malformed or does not answer `expected_id`.
pub(crate) fn parse_response(
    message: &[u8],
    expected_id: u16,
    expected_name: &str,
    qtype: QueryType,
) -> Option<DnsRecords> {
    if message.len() < 12 {
        return None;
    }
    let id = u16::from_be_bytes([message[0], message[1]]);
    if id != expected_id {
        return None;
    }
    let flags = u16::from_be_bytes([message[2], message[3]]);
    if flags & 0x8000 == 0 {
        return None; // not a response
    }
    let question_count = u16::from_be_bytes([message[4], message[5]]) as usize;
    let answer_count = u16::from_be_bytes([message[6], message[7]]) as usize;
    let mut records = DnsRecords::new(qtype);
    records.rcode = (flags & 0x000F) as u8;

    let mut pos = 12usize;
    let mut question_name: Option<NameBuf> = None;
    for index in 0..question_count {
        let decoded = NameBuf::decode(message, &mut pos)?;
        if index == 0 {
            question_name = Some(decoded);
        }
        pos = pos.checked_add(4)?;
    }
    if !question_name.is_some_and(|name| name.matches(expected_name)) {
        return None;
    }

    let mut saw_relevant = false;
    // Owner the data records are accepted from: starts as the queried name
    // and advances along same-packet CNAME links (recursive servers append
    // chain data after each link).
    let mut accepted_owner: NameBuf = match question_name {
        Some(name) => name,
        None => return None,
    };
    for _ in 0..answer_count {
        if pos >= message.len() {
            break;
        }
        let owner = NameBuf::decode(message, &mut pos)?;
        if pos + 10 > message.len() {
            return None;
        }
        let rtype = u16::from_be_bytes([message[pos], message[pos + 1]]);
        // class at +2..+4 ignored (IN assumed)
        let ttl_ms = u32::from_be_bytes([
            message[pos + 4],
            message[pos + 5],
            message[pos + 6],
            message[pos + 7],
        ]) as u64
            * 1000;
        let rdlength = u16::from_be_bytes([message[pos + 8], message[pos + 9]]) as usize;
        pos += 10;
        if pos + rdlength > message.len() {
            return None;
        }
        let rdata_start = pos;
        pos += rdlength;

        let owner_matches = owner.len == accepted_owner.len
            && owner.bytes[..owner.len] == accepted_owner.bytes[..accepted_owner.len];
        match rtype {
            RTYPE_A if rdlength == 4 => {
                if !owner_matches || records.a_count >= records.a.len() {
                    continue;
                }
                records.a[records.a_count] = u32::from_be_bytes([
                    message[rdata_start],
                    message[rdata_start + 1],
                    message[rdata_start + 2],
                    message[rdata_start + 3],
                ]);
                records.a_count += 1;
                fold_ttl(&mut records.min_ttl_ms, ttl_ms, saw_relevant);
                saw_relevant = true;
            }
            RTYPE_AAAA if rdlength == 16 => {
                if !owner_matches || records.aaaa_count >= records.aaaa.len() {
                    continue;
                }
                records.aaaa[records.aaaa_count]
                    .copy_from_slice(&message[rdata_start..rdata_start + 16]);
                records.aaaa_count += 1;
                fold_ttl(&mut records.min_ttl_ms, ttl_ms, saw_relevant);
                saw_relevant = true;
            }
            RTYPE_CNAME => {
                if !owner_matches {
                    continue;
                }
                if records.cname.is_none() {
                    let mut data_pos = rdata_start;
                    let target = NameBuf::decode(message, &mut data_pos)?;
                    records.cname = Some(target);
                }
                // Advance the accepted owner to the chain end so a fully
                // appended answer parses in a single pass.
                accepted_owner = records.cname.unwrap();
                records.resolved_owner = Some(accepted_owner);
                fold_ttl(&mut records.min_ttl_ms, ttl_ms, saw_relevant);
                saw_relevant = true;
            }
            RTYPE_TXT if rdlength > 0 => {
                if !owner_matches {
                    continue;
                }
                let mut taken = 0usize;
                let mut cursor = rdata_start;
                let end = rdata_start + rdlength;
                while cursor < end {
                    let part_len = message[cursor] as usize;
                    cursor += 1;
                    if cursor + part_len > end {
                        return None;
                    }
                    let copy_len = part_len.min(MAX_TXT_BYTES.saturating_sub(taken));
                    if copy_len > 0 {
                        records.txt[taken..taken + copy_len]
                            .copy_from_slice(&message[cursor..cursor + copy_len]);
                        taken += copy_len;
                    }
                    cursor += part_len;
                }
                records.txt_len = taken;
                fold_ttl(&mut records.min_ttl_ms, ttl_ms, saw_relevant);
                saw_relevant = true;
            }
            _ => {}
        }
    }
    if !saw_relevant && records.cname.is_none() {
        records.min_ttl_ms = 0;
    }
    Some(records)
}

fn fold_ttl(min_ttl: &mut u64, ttl_ms: u64, already_started: bool) {
    if !already_started || ttl_ms < *min_ttl {
        *min_ttl = ttl_ms;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_name(name: &str, out: &mut Vec<u8>) {
        for label in name.split('.') {
            out.push(label.len() as u8);
            out.extend_from_slice(label.as_bytes());
        }
        out.push(0);
    }

    fn header(id: u16, rcode: u8, answer_records: usize) -> Vec<u8> {
        header_q(id, rcode, answer_records, "example.test", RTYPE_A)
    }

    fn header_q(
        id: u16,
        rcode: u8,
        answer_records: usize,
        qname: &str,
        qtype_rtype: u16,
    ) -> Vec<u8> {
        let mut msg = Vec::new();
        msg.extend_from_slice(&id.to_be_bytes());
        msg.extend_from_slice(&(0x8000u16 | rcode as u16).to_be_bytes());
        msg.extend_from_slice(&1u16.to_be_bytes());
        msg.extend_from_slice(&(answer_records as u16).to_be_bytes());
        msg.extend_from_slice(&0u16.to_be_bytes());
        msg.extend_from_slice(&0u16.to_be_bytes());
        encode_name(qname, &mut msg);
        msg.extend_from_slice(&qtype_rtype.to_be_bytes());
        msg.extend_from_slice(&1u16.to_be_bytes());
        msg
    }

    #[test]
    fn query_round_trip() {
        let mut buf = [0u8; 128];
        let len = build_query(&mut buf, 0x1234, "a.b.c", QueryType::A).unwrap();
        assert_eq!(len, 12 + 7 + 4, "header + labels(a.b.c)+root + type+class");
        let mut pos = 12usize;
        let decoded = NameBuf::decode(&buf[..len], &mut pos).unwrap();
        assert!(decoded.matches("a.b.c"));
        assert_eq!((buf[len - 4], buf[len - 3]), (0, 1));
    }

    #[test]
    fn parse_a_answer_with_compression() {
        let mut msg = header(0x00aa, RCODE_OK, 2);
        // Answer 1: CNAME example.test -> target.test (target name inline).
        encode_name("example.test", &mut msg);
        msg.extend_from_slice(&RTYPE_CNAME.to_be_bytes());
        msg.extend_from_slice(&1u16.to_be_bytes());
        msg.extend_from_slice(&60u32.to_be_bytes());
        let mut rdata = Vec::new();
        encode_name("target.test", &mut rdata);
        msg.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
        let target_offset = msg.len() as u16;
        msg.extend_from_slice(&rdata);
        // Answer 2: compression pointer into the CNAME rdata (target.test),
        // then its A record -- mirrors how resolvers pack chain answers.
        msg.push(0xC0 | ((target_offset >> 8) as u8));
        msg.push((target_offset & 0xff) as u8);
        msg.extend_from_slice(&RTYPE_A.to_be_bytes());
        msg.extend_from_slice(&1u16.to_be_bytes());
        msg.extend_from_slice(&120u32.to_be_bytes());
        msg.extend_from_slice(&4u16.to_be_bytes());
        msg.extend_from_slice(&[10, 1, 2, 3]);

        let records = parse_response(&msg, 0x00aa, "example.test", QueryType::A).unwrap();
        assert_eq!(records.rcode, RCODE_OK);
        // The CNAME advances the accepted owner to target.test, so the
        // appended A record for it is accepted in the same pass.
        assert_eq!(records.a_count, 1);
        assert_eq!(records.a[0], u32::from_be_bytes([10, 1, 2, 3]));
        assert!(records.cname.unwrap().matches("target.test"));
        assert!(records.resolved_owner.unwrap().matches("target.test"));
        assert_eq!(records.min_ttl_ms, 60_000);

        // Same packet but queried against the chain target yields nothing:
        // the question section still names example.test.
        let records = parse_response(&msg, 0x00aa, "target.test", QueryType::A);
        assert!(records.is_none());
    }

    #[test]
    fn parse_a_direct_answer() {
        let mut msg = header(7, RCODE_OK, 1);
        encode_name("example.test", &mut msg);
        msg.extend_from_slice(&RTYPE_A.to_be_bytes());
        msg.extend_from_slice(&1u16.to_be_bytes());
        msg.extend_from_slice(&30u32.to_be_bytes());
        msg.extend_from_slice(&4u16.to_be_bytes());
        msg.extend_from_slice(&[192, 0, 2, 1]);
        let records = parse_response(&msg, 7, "example.test", QueryType::A).unwrap();
        assert_eq!(records.a_count, 1);
        assert_eq!(records.a[0], u32::from_be_bytes([192, 0, 2, 1]));
        assert_eq!(records.min_ttl_ms, 30_000);
        assert!(records.cname.is_none());
    }

    #[test]
    fn parse_rcode_nxdomain_and_servfail() {
        let msg = header(9, RCODE_NXDOMAIN, 0);
        let records = parse_response(&msg, 9, "example.test", QueryType::A).unwrap();
        assert_eq!(records.rcode, RCODE_NXDOMAIN);
        let msg = header(9, RCODE_SERVFAIL, 0);
        let records = parse_response(&msg, 9, "example.test", QueryType::A).unwrap();
        assert_eq!(records.rcode, RCODE_SERVFAIL);
    }

    #[test]
    fn parse_txt_multi_string() {
        let mut msg = header(11, RCODE_OK, 1);
        encode_name("example.test", &mut msg);
        msg.extend_from_slice(&RTYPE_TXT.to_be_bytes());
        msg.extend_from_slice(&1u16.to_be_bytes());
        msg.extend_from_slice(&300u32.to_be_bytes());
        let parts: [&[u8]; 2] = [b"hello ", b"world"];
        let rdlen: usize = parts.iter().map(|p| p.len() + 1).sum();
        msg.extend_from_slice(&(rdlen as u16).to_be_bytes());
        for part in parts {
            msg.push(part.len() as u8);
            msg.extend_from_slice(part);
        }
        let records = parse_response(&msg, 11, "example.test", QueryType::Txt).unwrap();
        assert_eq!(records.rcode, RCODE_OK);
        assert_eq!(records.txt_len, "hello world".len());
        assert_eq!(&records.txt[..records.txt_len], b"hello world");
        assert_eq!(records.min_ttl_ms, 300_000);
    }

    #[test]
    fn parse_aaaa_answer() {
        let mut msg = header_q(12, RCODE_OK, 1, "v6.example.test", RTYPE_AAAA);
        encode_name("v6.example.test", &mut msg);
        msg.extend_from_slice(&RTYPE_AAAA.to_be_bytes());
        msg.extend_from_slice(&1u16.to_be_bytes());
        msg.extend_from_slice(&45u32.to_be_bytes());
        msg.extend_from_slice(&16u16.to_be_bytes());
        let addr: [u8; 16] = core::array::from_fn(|i| i as u8);
        msg.extend_from_slice(&addr);
        let records = parse_response(&msg, 12, "v6.example.test", QueryType::Aaaa).unwrap();
        assert_eq!(records.aaaa_count, 1);
        assert_eq!(records.aaaa[0], addr);
        assert_eq!(records.min_ttl_ms, 45_000);
    }

    #[test]
    fn rejects_wrong_id_short_packet_and_non_response() {
        let msg = header(1, RCODE_OK, 0);
        assert!(parse_response(&msg, 2, "example.test", QueryType::A).is_none());
        assert!(parse_response(&[], 1, "example.test", QueryType::A).is_none());
        let mut not_response = header(1, RCODE_OK, 0);
        not_response[2] = 0; // clear QR bit
        assert!(parse_response(&not_response, 1, "example.test", QueryType::A).is_none());
    }

    #[test]
    fn name_buf_parse_bounds() {
        assert!(NameBuf::parse("").is_none());
        assert!(NameBuf::parse("a..b").is_none());
        let long = "x".repeat(MAX_RESOLVER_NAME_BYTES + 1);
        assert!(NameBuf::parse(&long).is_none());
        assert!(NameBuf::parse("ok.name").is_some());
    }
}
