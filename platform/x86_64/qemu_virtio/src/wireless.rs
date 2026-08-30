//! Platform-neutral Wi-Fi control-plane protocol layer (virtio-wlan-shaped,
//! CFG80211-style command envelopes).
//!
//! Pure logic only: no MMIO, no probing, no IRQ paths. A future virtio-wlan
//! (or other 802.11) device backend would stage these byte shapes through its
//! own transport and implement the kernel's `WirelessBackend` contract.
//! Dependency-free apart from `serviceos-crypto` (SHA-512) so the whole
//! module can be included by path in the host test harness, exactly like the
//! raspi5 mailbox driver.
//!
//! Wire surface:
//! - command envelopes: `[cmd u16 LE][seq u16 LE][attributes...]`, each
//!   attribute `[id u8][len u8][payload...]` (nl80211/CFG80211-shaped);
//! - responses: `[status u16 LE][attributes...]`;
//! - scan records: `[rssi i8][channel u8][body_len u16 LE][802.11 mgmt
//!   body]` with IE decode for SSID (0), DS parameter set (3) and the RSNE
//!   (48) driving an open/WPA2/WPA3 classification;
//! - saved-network store codec: magic/version/count + per-record
//!   ssid/psk/bssid/priority, fully capacity-bounded;
//! - EAPOL key frames (messages 1-4) with the WPA2-PSK 4-way handshake as an
//!   authenticator-side state machine.
//!
//! UNTESTED WITHOUT HARDWARE: nothing in this module has executed against a
//! real 802.11 NIC or a virtio-wlan device model. What would validate it:
//! a live scan/join capture replayed through `decode_scan_record` and the
//! envelope parser, plus a real 4-way-handshake capture driven through
//! `Authenticator`. The cryptographic layer is explicitly integrity-grade —
//! see the placeholder notes on `hmac_sha512`, `derive_ptk_placeholder`,
//! `eapol_mic_placeholder` and `pmk_from_psk_placeholder`; it proves the
//! state-machine plumbing, not real WPA2 security, and MUST be replaced
//! before any hardware bring-up.
#![allow(dead_code)]

use serviceos_crypto::sha512::Sha512;

/// Maximum encoded command/response envelope size (bytes).
pub const MAX_ENVELOPE_BYTES: usize = 256;
/// Maximum SSID octet length per 802.11.
pub const MAX_SSID_LEN: usize = 32;
/// Maximum PSK octet length (64 for raw passphrases).
pub const MAX_PSK_LEN: usize = 64;
/// Saved-network slots (no heap; store is fixed-capacity).
pub const MAX_SAVED_NETWORKS: usize = 8;

// ---------------------------------------------------------------------------
// Command envelope (CFG80211-style)
// ---------------------------------------------------------------------------

/// Trigger an off-channel scan.
pub const CMD_TRIGGER_SCAN: u16 = 1;
/// Fetch accumulated scan results (device answers with a status envelope;
/// records arrive as events).
pub const CMD_SCAN_RESULTS: u16 = 2;
/// Join a network: full scan→auth→associate sequence driven by the device.
pub const CMD_JOIN: u16 = 3;
/// Authenticate with a specific BSSID only.
pub const CMD_AUTHENTICATE: u16 = 4;
/// Associate with an authenticated BSSID.
pub const CMD_ASSOCIATE: u16 = 5;
/// Drop the current link.
pub const CMD_DISCONNECT: u16 = 6;

/// Response status: accepted.
pub const STATUS_OK: u16 = 0;
/// Response status: rejected (bad parameters or wrong link state).
pub const STATUS_REJECTED: u16 = 1;

pub const ATTR_SSID: u8 = 1;
pub const ATTR_BSSID: u8 = 2;
pub const ATTR_CHANNEL: u8 = 3;
pub const ATTR_PSK: u8 = 4;
pub const ATTR_PRIORITY: u8 = 5;
pub const ATTR_TIMEOUT_MS: u8 = 6;
pub const ATTR_SECURITY: u8 = 7;

/// Errors surfaced by the envelope and record parsers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParseError {
    /// Buffer shorter than the fixed header requires.
    TooShort,
    /// An attribute header or payload runs past the envelope.
    AttrTruncated,
    /// Attribute length leaves the payload out of bounds.
    BadAttrLength,
}

/// Incremental builder for command envelopes over a fixed buffer.
pub struct CommandBuilder {
    buffer: [u8; MAX_ENVELOPE_BYTES],
    used: usize,
}

impl CommandBuilder {
    /// Starts an envelope for `cmd` with the given sequence number.
    pub fn new(command: u16, sequence: u16) -> CommandBuilder {
        let mut builder = CommandBuilder {
            buffer: [0; MAX_ENVELOPE_BYTES],
            used: 0,
        };
        builder.buffer[..2].copy_from_slice(&command.to_le_bytes());
        builder.buffer[2..4].copy_from_slice(&sequence.to_le_bytes());
        builder.used = 4;
        builder
    }

    /// Appends one attribute. Rejects overflow and payloads that cannot be
    /// length-encoded (the wire length field is one byte).
    pub fn attr(&mut self, id: u8, payload: &[u8]) -> Result<(), ParseError> {
        if payload.len() > u8::MAX as usize {
            return Err(ParseError::BadAttrLength);
        }
        let need = 2 + payload.len();
        if self.used + need > MAX_ENVELOPE_BYTES {
            return Err(ParseError::TooShort);
        }
        self.buffer[self.used] = id;
        self.buffer[self.used + 1] = payload.len() as u8;
        self.buffer[self.used + 2..self.used + need].copy_from_slice(payload);
        self.used += need;
        Ok(())
    }

    /// Convenience: SSID attribute.
    pub fn ssid(&mut self, ssid: &[u8]) -> Result<(), ParseError> {
        if ssid.len() > MAX_SSID_LEN {
            return Err(ParseError::BadAttrLength);
        }
        self.attr(ATTR_SSID, ssid)
    }

    /// Convenience: BSSID attribute.
    pub fn bssid(&mut self, bssid: &[u8; 6]) -> Result<(), ParseError> {
        self.attr(ATTR_BSSID, bssid)
    }

    /// Convenience: channel attribute.
    pub fn channel(&mut self, channel: u8) -> Result<(), ParseError> {
        self.attr(ATTR_CHANNEL, &[channel])
    }

    /// Convenience: PSK attribute.
    pub fn psk(&mut self, psk: &[u8]) -> Result<(), ParseError> {
        if psk.len() > MAX_PSK_LEN {
            return Err(ParseError::BadAttrLength);
        }
        self.attr(ATTR_PSK, psk)
    }

    /// The encoded envelope bytes.
    pub fn finish(&self) -> &[u8] {
        &self.buffer[..self.used]
    }
}

/// Iterator over the attribute section of an envelope.
pub struct AttrIter<'a> {
    bytes: &'a [u8],
}

impl<'a> Iterator for AttrIter<'a> {
    type Item = (u8, &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        if self.bytes.len() < 2 {
            return None;
        }
        let id = self.bytes[0];
        let len = self.bytes[1] as usize;
        if self.bytes.len() < 2 + len {
            // Malformed tail: stop emitting rather than panicking; the
            // top-level parser flags this via a separate length check.
            self.bytes = &[];
            return None;
        }
        let payload = &self.bytes[2..2 + len];
        self.bytes = &self.bytes[2 + len..];
        Some((id, payload))
    }
}

/// A parsed response envelope borrowing its attribute bytes.
pub struct Response<'a> {
    /// Device response status ([`STATUS_OK`] / [`STATUS_REJECTED`]).
    pub status: u16,
    attrs: &'a [u8],
}

impl<'a> Response<'a> {
    /// Iterates the attributes in declaration order.
    pub fn attrs(&self) -> AttrIter<'a> {
        AttrIter { bytes: self.attrs }
    }

    /// First attribute payload with the given id, if present.
    pub fn find(&self, id: u8) -> Option<&'a [u8]> {
        self.attrs()
            .find(|(attr_id, _)| *attr_id == id)
            .map(|(_, p)| p)
    }
}

/// Parses a response envelope: `[status u16 LE][attributes...]`.
pub fn parse_response(bytes: &[u8]) -> Result<Response<'_>, ParseError> {
    if bytes.len() < 2 {
        return Err(ParseError::TooShort);
    }
    let status = u16::from_le_bytes([bytes[0], bytes[1]]);
    let attrs = &bytes[2..];
    // Reject truncated attribute tails outright so callers can distinguish
    // "no attributes" from "corrupt envelope".
    let mut cursor = attrs;
    while !cursor.is_empty() {
        if cursor.len() < 2 {
            return Err(ParseError::AttrTruncated);
        }
        let len = cursor[1] as usize;
        if cursor.len() < 2 + len {
            return Err(ParseError::BadAttrLength);
        }
        cursor = &cursor[2 + len..];
    }
    Ok(Response { status, attrs })
}

// ---------------------------------------------------------------------------
// Scan-record decode (beacon / probe response)
// ---------------------------------------------------------------------------

/// Security classification derived from the RSNE (or its absence).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Security {
    /// No RSNE present.
    Open,
    /// RSNE present, no SAE AKM suite.
    Wpa2,
    /// RSNE present advertising SAE (AKM suite type 8).
    Wpa3,
    /// RSNE present but malformed; treat as unusable rather than open.
    Unknown,
}

/// Decode failures for scan records.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeError {
    /// Record header (rssi/channel/length) or body truncated.
    TooShort,
    /// Declared body length does not match the bytes present.
    BadBodyLength,
    /// Frame control does not describe a management frame.
    NotManagementFrame,
    /// Fixed 802.11 management fields incomplete.
    BadFixedFields,
    /// An information element runs past the end of the body.
    IeTruncated,
}

/// One decoded beacon/probe-response scan record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScanEntry<'a> {
    /// Transmitter address of the frame (BSSID in infrastructure networks).
    pub bssid: [u8; 6],
    /// Signal quality reported by the device (device units / dBm-ish byte).
    pub rssi: i8,
    /// Operating channel (from the DS parameter set IE when present).
    pub channel: u8,
    /// Decoded SSID octets (may be empty for wildcard beacons).
    pub ssid: &'a [u8],
    /// Security classification from RSNE presence and AKM suites.
    pub security: Security,
}

/// Decodes one device-shaped scan record:
/// `[rssi i8][channel u8][body_len u16 LE][802.11 mgmt body]`.
///
/// The management body is the 802.11 MAC header + fixed fields + IEs of a
/// beacon or probe response; BSSID is taken from address 3.
pub fn decode_scan_record(record: &[u8]) -> Result<ScanEntry<'_>, DecodeError> {
    if record.len() < 4 {
        return Err(DecodeError::TooShort);
    }
    let rssi = record[0] as i8;
    let channel = record[1];
    let body_len = u16::from_le_bytes([record[2], record[3]]) as usize;
    if record.len() - 4 < body_len {
        return Err(DecodeError::BadBodyLength);
    }
    let body = &record[4..4 + body_len];

    // MAC header: fc(2) duration(2) addr1(6) addr2(6) addr3(6) seq-ctl(2)
    // = 24 bytes; the BSSID is address 3 in infrastructure frames.
    if body.len() < 24 {
        return Err(DecodeError::BadFixedFields);
    }
    let frame_control = u16::from_le_bytes([body[0], body[1]]);
    if frame_control & 0x000C != 0 {
        return Err(DecodeError::NotManagementFrame);
    }
    let mut bssid = [0u8; 6];
    bssid.copy_from_slice(&body[16..22]);

    // Fixed fields: timestamp(8) beacon-interval(2) capability(2).
    if body.len() < 36 {
        return Err(DecodeError::BadFixedFields);
    }

    let mut entry = ScanEntry {
        bssid,
        rssi,
        channel,
        ssid: &[],
        security: Security::Open,
    };

    let mut cursor = &body[36..];
    while !cursor.is_empty() {
        if cursor.len() < 2 {
            return Err(DecodeError::IeTruncated);
        }
        let ie_id = cursor[0];
        let ie_len = cursor[1] as usize;
        if cursor.len() < 2 + ie_len {
            return Err(DecodeError::IeTruncated);
        }
        let payload = &cursor[2..2 + ie_len];
        match ie_id {
            0 => {
                if payload.len() > MAX_SSID_LEN {
                    return Err(DecodeError::IeTruncated);
                }
                entry.ssid = payload;
            }
            3 => {
                if ie_len != 1 {
                    return Err(DecodeError::IeTruncated);
                }
                entry.channel = payload[0];
            }
            48 => entry.security = classify_rsne(payload),
            _ => {}
        }
        cursor = &cursor[2 + ie_len..];
    }
    Ok(entry)
}

/// Classifies an RSNE payload: absent SAE AKM → [`Security::Wpa2`], SAE AKM
/// suite type 8 → [`Security::Wpa3`], malformed → [`Security::Unknown`].
///
/// Layout (802.11 RSNE): version(u16 LE), group cipher(4), pairwise count
/// (u16 LE), pairwise suites (4*n), AKM count (u16 LE), AKM suites (4*n).
/// Suite selectors are `00:0F:AC:<type>`; the suite type lives in the high
/// byte of the little-endian u32.
fn classify_rsne(rsne: &[u8]) -> Security {
    if rsne.len() < 2 {
        return Security::Unknown;
    }
    let version = u16::from_le_bytes([rsne[0], rsne[1]]);
    if version != 1 {
        return Security::Unknown;
    }
    let mut cursor = &rsne[2..];
    // Group cipher suite.
    if cursor.len() < 4 {
        return Security::Unknown;
    }
    cursor = &cursor[4..];
    // Pairwise cipher suite list.
    if cursor.len() < 2 {
        return Security::Unknown;
    }
    let pairwise_count = u16::from_le_bytes([cursor[0], cursor[1]]) as usize;
    cursor = &cursor[2..];
    if pairwise_count > 8 || cursor.len() < pairwise_count * 4 {
        return Security::Unknown;
    }
    cursor = &cursor[pairwise_count * 4..];
    // AKM suite list.
    if cursor.len() < 2 {
        return Security::Unknown;
    }
    let akm_count = u16::from_le_bytes([cursor[0], cursor[1]]) as usize;
    cursor = &cursor[2..];
    if akm_count > 8 || cursor.len() < akm_count * 4 {
        return Security::Unknown;
    }
    for suite in cursor.chunks_exact(4) {
        // SAE = AKM suite type 8 inside selector 00:0F:AC.
        if suite[3] == 8 {
            return Security::Wpa3;
        }
    }
    Security::Wpa2
}

// ---------------------------------------------------------------------------
// Saved-network store (capacity-bounded, heap-free)
// ---------------------------------------------------------------------------

/// Store codec failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodecError {
    /// Destination buffer too small for the encoded store.
    BufferTooSmall,
    /// Source bytes do not start with the store magic.
    BadMagic,
    /// Unsupported codec version.
    BadVersion,
    /// Record section truncated or count exceeds capacity.
    BadRecord,
    /// Record fields (ssid/psk lengths) violate the wire limits.
    BadField,
}

/// One remembered network.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SavedNetwork {
    pub ssid: [u8; MAX_SSID_LEN],
    pub ssid_len: usize,
    pub psk: [u8; MAX_PSK_LEN],
    pub psk_len: usize,
    /// Preferred BSSID when the user pinned one; `None` = any BSSID.
    pub bssid: Option<[u8; 6]>,
    /// Higher wins when several saved networks are visible.
    pub priority: u8,
}

impl SavedNetwork {
    /// Builds a record, rejecting oversized fields.
    pub fn new(
        ssid: &[u8],
        psk: &[u8],
        bssid: Option<[u8; 6]>,
        priority: u8,
    ) -> Option<SavedNetwork> {
        if ssid.is_empty() || ssid.len() > MAX_SSID_LEN || psk.is_empty() || psk.len() > MAX_PSK_LEN
        {
            return None;
        }
        let mut record = SavedNetwork {
            ssid: [0; MAX_SSID_LEN],
            ssid_len: ssid.len(),
            psk: [0; MAX_PSK_LEN],
            psk_len: psk.len(),
            bssid,
            priority,
        };
        record.ssid[..ssid.len()].copy_from_slice(ssid);
        record.psk[..psk.len()].copy_from_slice(psk);
        Some(record)
    }

    /// SSID octets.
    pub fn ssid_bytes(&self) -> &[u8] {
        &self.ssid[..self.ssid_len]
    }

    /// PSK octets.
    pub fn psk_bytes(&self) -> &[u8] {
        &self.psk[..self.psk_len]
    }
}

/// Fixed-capacity saved-network store.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedNetworkStore {
    entries: [Option<SavedNetwork>; MAX_SAVED_NETWORKS],
    count: usize,
}

impl SavedNetworkStore {
    /// Empty store.
    pub fn new() -> SavedNetworkStore {
        SavedNetworkStore {
            entries: Default::default(),
            count: 0,
        }
    }

    /// Number of stored networks.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Insert or update a network. Same-SSID records are replaced; a new
    /// SSID beyond capacity is rejected ([`CodecError::BadRecord`] via
    /// `None`).
    pub fn insert(&mut self, record: SavedNetwork) -> Option<()> {
        for slot in self.entries.iter_mut().take(self.count) {
            if let Some(existing) = slot {
                if existing.ssid_bytes() == record.ssid_bytes() {
                    *slot = Some(record);
                    return Some(());
                }
            }
        }
        if self.count >= MAX_SAVED_NETWORKS {
            return None;
        }
        self.entries[self.count] = Some(record);
        self.count += 1;
        Some(())
    }

    /// Removes the record with the given SSID; `true` when removed.
    pub fn remove(&mut self, ssid: &[u8]) -> bool {
        for index in 0..self.count {
            if let Some(existing) = self.entries[index] {
                if existing.ssid_bytes() == ssid {
                    // Compact in place to keep the store dense.
                    for shift in index..self.count - 1 {
                        self.entries[shift] = self.entries[shift + 1];
                    }
                    self.entries[self.count - 1] = None;
                    self.count -= 1;
                    return true;
                }
            }
        }
        false
    }

    /// Highest-priority record (ties resolve to the earliest inserted).
    pub fn best(&self) -> Option<&SavedNetwork> {
        let mut best: Option<&SavedNetwork> = None;
        for slot in self.entries.iter().take(self.count) {
            if let Some(record) = slot {
                if best.is_none_or(|current| record.priority > current.priority) {
                    best = Some(record);
                }
            }
        }
        best
    }

    /// Iterates stored records in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = &SavedNetwork> {
        self.entries
            .iter()
            .take(self.count)
            .map(|slot| slot.as_ref().expect("dense store"))
    }

    /// Encodes the store: `[magic u16 LE][version u8][count u8][records...]`,
    /// each record `[ssid_len u8][ssid][psk_len u8][psk][bssid_flag u8]
    /// [bssid 6 when flagged][priority u8]`.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, CodecError> {
        let mut used = 4usize;
        for record in self.iter() {
            used += 1 + record.ssid_len + 1 + record.psk_len + 1 + 1;
            if record.bssid.is_some() {
                used += 6;
            }
        }
        if out.len() < used {
            return Err(CodecError::BufferTooSmall);
        }
        out[..2].copy_from_slice(&STORE_MAGIC.to_le_bytes());
        out[2] = STORE_VERSION;
        out[3] = self.count as u8;
        let mut offset = 4usize;
        for record in self.iter() {
            out[offset] = record.ssid_len as u8;
            offset += 1;
            out[offset..offset + record.ssid_len].copy_from_slice(record.ssid_bytes());
            offset += record.ssid_len;
            out[offset] = record.psk_len as u8;
            offset += 1;
            out[offset..offset + record.psk_len].copy_from_slice(record.psk_bytes());
            offset += record.psk_len;
            match record.bssid {
                Some(bssid) => {
                    out[offset] = 1;
                    offset += 1;
                    out[offset..offset + 6].copy_from_slice(&bssid);
                    offset += 6;
                }
                None => {
                    out[offset] = 0;
                    offset += 1;
                }
            }
            out[offset] = record.priority;
            offset += 1;
        }
        Ok(used)
    }

    /// Decodes a store previously written by [`Self::encode`]. Strict about
    /// magic, version, capacity and field bounds.
    pub fn decode(bytes: &[u8]) -> Result<SavedNetworkStore, CodecError> {
        if bytes.len() < 4 {
            return Err(CodecError::BadRecord);
        }
        let magic = u16::from_le_bytes([bytes[0], bytes[1]]);
        if magic != STORE_MAGIC {
            return Err(CodecError::BadMagic);
        }
        if bytes[2] != STORE_VERSION {
            return Err(CodecError::BadVersion);
        }
        let count = bytes[3] as usize;
        if count > MAX_SAVED_NETWORKS {
            return Err(CodecError::BadRecord);
        }
        let mut store = SavedNetworkStore::new();
        let mut cursor = &bytes[4..];
        for _ in 0..count {
            let record = read_record(&mut cursor)?;
            store.insert(record).ok_or(CodecError::BadRecord)?;
        }
        if !cursor.is_empty() {
            return Err(CodecError::BadRecord);
        }
        Ok(store)
    }
}

impl Default for SavedNetworkStore {
    fn default() -> Self {
        Self::new()
    }
}

const STORE_MAGIC: u16 = 0x5357;
const STORE_VERSION: u8 = 1;

fn read_record(cursor: &mut &[u8]) -> Result<SavedNetwork, CodecError> {
    if cursor.len() < 1 {
        return Err(CodecError::BadRecord);
    }
    let ssid_len = cursor[0] as usize;
    *cursor = &cursor[1..];
    if ssid_len == 0 || ssid_len > MAX_SSID_LEN || cursor.len() < ssid_len {
        return Err(CodecError::BadField);
    }
    let ssid = &cursor[..ssid_len];
    *cursor = &cursor[ssid_len..];
    if cursor.len() < 1 {
        return Err(CodecError::BadRecord);
    }
    let psk_len = cursor[0] as usize;
    *cursor = &cursor[1..];
    if psk_len == 0 || psk_len > MAX_PSK_LEN || cursor.len() < psk_len {
        return Err(CodecError::BadField);
    }
    let psk = &cursor[..psk_len];
    *cursor = &cursor[psk_len..];
    if cursor.len() < 1 {
        return Err(CodecError::BadRecord);
    }
    let has_bssid = cursor[0] != 0;
    *cursor = &cursor[1..];
    let bssid = if has_bssid {
        if cursor.len() < 6 {
            return Err(CodecError::BadRecord);
        }
        let mut value = [0u8; 6];
        value.copy_from_slice(&cursor[..6]);
        *cursor = &cursor[6..];
        Some(value)
    } else {
        None
    };
    if cursor.is_empty() {
        return Err(CodecError::BadRecord);
    }
    let priority = cursor[0];
    *cursor = &cursor[1..];
    SavedNetwork::new(ssid, psk, bssid, priority).ok_or(CodecError::BadField)
}

// ---------------------------------------------------------------------------
// Integrity-grade key material (HMAC / PRF placeholders)
// ---------------------------------------------------------------------------

/// HMAC-SHA-512 (RFC 2104 block layout, block size 128).
///
/// Built over `serviceos-crypto`'s SHA-512 because the workspace has no
/// other hash. INTEGRITY-GRADE HONESTY NOTE: real WPA2 key derivation uses
/// HMAC-SHA-1 (PRF-512, 802.11i) and AES-CMAC MICs (802.11w/CCMP); this
/// SHA-512 HMAC is a *placeholder* of the same shape so the protocol layer
/// is complete and testable. It is cryptographically sound as an HMAC but
/// is NOT interoperable with real 802.11i peers, and MUST be replaced with
/// the spec algorithms before hardware bring-up.
pub fn hmac_sha512(key: &[u8], parts: &[&[u8]]) -> [u8; 64] {
    const BLOCK: usize = 128;
    let mut key_block = [0u8; BLOCK];
    if key.len() > BLOCK {
        // Long keys are hashed first (RFC 2104).
        let mut hash = Sha512::new();
        hash.update(key);
        let long_digest = hash.finalize();
        key_block[..64].copy_from_slice(&long_digest);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for index in 0..BLOCK {
        ipad[index] ^= key_block[index];
        opad[index] ^= key_block[index];
    }
    let mut inner = Sha512::new();
    inner.update(&ipad);
    for part in parts {
        inner.update(part);
    }
    let inner_digest = inner.finalize();
    let mut outer = Sha512::new();
    outer.update(&opad);
    outer.update(&inner_digest);
    outer.finalize()
}

/// Placeholder PMK derivation from a PSK.
///
/// INTEGRITY-GRADE HONESTY NOTE: real WPA2-PSK computes
/// `PMK = PBKDF2-HMAC-SHA1(psk, ssid, 4096 iterations, 32 bytes)`. This
/// placeholder truncates `SHA-512(psk || ssid)` to 32 bytes — same output
/// size and binding, different KDF — and MUST be replaced before hardware.
pub fn pmk_from_psk_placeholder(psk: &[u8], ssid: &[u8]) -> [u8; 32] {
    let mut hash = Sha512::new();
    hash.update(psk);
    hash.update(ssid);
    let digest = hash.finalize();
    let mut pmk = [0u8; 32];
    pmk.copy_from_slice(&digest[..32]);
    pmk
}

/// CCMP-shaped 48-byte pairwise transient key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ptk {
    /// Key confirmation key (MIC computations).
    pub kck: [u8; 16],
    /// Key encryption key (GTK delivery).
    pub kek: [u8; 16],
    /// Temporal key (data confidentiality).
    pub tk: [u8; 16],
}

/// Placeholder PTK derivation.
///
/// INTEGRITY-GRADE HONESTY NOTE: the 802.11i construction is
/// `PTK = PRF-512(PMK, "Pairwise key expansion", min/max(AA,SPA) ||
/// min/max(ANonce,SNonce))` using HMAC-SHA-1, then split KCK|KEK|TK. This
/// placeholder keeps the label, address binding and CCMP 48-byte split but
/// uses a single HMAC-SHA-512 block truncated to 48 bytes. It MUST be
/// replaced with the spec PRF before hardware.
pub fn derive_ptk_placeholder(
    pmk: &[u8; 32],
    aa: &[u8; 6],
    spa: &[u8; 6],
    anonce: &[u8; 32],
    snonce: &[u8; 32],
) -> Ptk {
    let digest = hmac_sha512(pmk, &[b"Pairwise key expansion", aa, spa, anonce, snonce]);
    let mut kck = [0u8; 16];
    kck.copy_from_slice(&digest[..16]);
    let mut kek = [0u8; 16];
    kek.copy_from_slice(&digest[16..32]);
    let mut tk = [0u8; 16];
    tk.copy_from_slice(&digest[32..48]);
    Ptk { kck, kek, tk }
}

/// Placeholder EAPOL MIC.
///
/// INTEGRITY-GRADE HONESTY NOTE: real CCMP MICs are AES-CMAC-128 over the
/// EAPOL frame with the MIC slot zeroed; TKIP uses HMAC-MD5. This
/// placeholder returns the first 16 bytes of HMAC-SHA-512 over the MIC
/// coverage (kind byte, replay counter, payload). It proves the MIC-slot
/// plumbing but MUST be replaced with AES-CMAC before hardware.
pub fn eapol_mic_placeholder(kck: &[u8; 16], covered: &[&[u8]]) -> [u8; 16] {
    let digest = hmac_sha512(kck, covered);
    let mut mic = [0u8; 16];
    mic.copy_from_slice(&digest[..16]);
    mic
}

// ---------------------------------------------------------------------------
// EAPOL key frames (wire shapes)
// ---------------------------------------------------------------------------

/// EAPOL key message types carried in the frame's kind byte.
pub const EAPOL_MESSAGE_1: u8 = 1;
pub const EAPOL_MESSAGE_2: u8 = 2;
pub const EAPOL_MESSAGE_3: u8 = 3;
pub const EAPOL_MESSAGE_4: u8 = 4;

/// Wire errors for EAPOL key frames.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireError {
    /// Frame shorter than its kind mandates.
    TooShort,
    /// Payload length field disagrees with the bytes present.
    BadPayloadLength,
    /// Destination buffer too small for the encoded frame.
    BufferTooSmall,
    /// Unknown message kind.
    BadKind,
}

/// One EAPOL key frame (message 1-4), decoded view with owned arrays.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EapolKeyFrame {
    pub kind: u8,
    /// Replay counter (big-endian on the wire).
    pub replay: u64,
    /// ANonce (messages 1) / SNonce (message 2).
    pub nonce: Option<[u8; 32]>,
    /// MIC slot (messages 2-4).
    pub mic: Option<[u8; 16]>,
    /// Key data payload covered by the MIC.
    pub payload_len: usize,
}

impl EapolKeyFrame {
    /// Encodes: `[kind u8][replay u64 BE][nonce 32 when kind 1|2]
    /// [mic 16 when kind 2|3|4][payload_len u16 LE][payload...]`.
    pub fn encode(&self, payload: &[u8], out: &mut [u8]) -> Result<usize, WireError> {
        let mut used = 9usize; // kind + replay
        match self.kind {
            EAPOL_MESSAGE_1 | EAPOL_MESSAGE_2 => {
                self.nonce.as_ref().ok_or(WireError::BadKind)?;
                used += 32;
            }
            EAPOL_MESSAGE_3 | EAPOL_MESSAGE_4 => {}
            _ => return Err(WireError::BadKind),
        }
        if self.kind >= EAPOL_MESSAGE_2 {
            self.mic.as_ref().ok_or(WireError::BadKind)?;
            used += 16;
        }
        if payload.len() > u16::MAX as usize {
            return Err(WireError::BadPayloadLength);
        }
        used += 2 + payload.len();
        if out.len() < used {
            return Err(WireError::BufferTooSmall);
        }
        out[0] = self.kind;
        out[1..9].copy_from_slice(&self.replay.to_be_bytes());
        let mut offset = 9usize;
        if let Some(nonce) = &self.nonce {
            if self.kind == EAPOL_MESSAGE_1 || self.kind == EAPOL_MESSAGE_2 {
                out[offset..offset + 32].copy_from_slice(nonce);
                offset += 32;
            }
        }
        if let Some(mic) = &self.mic {
            out[offset..offset + 16].copy_from_slice(mic);
            offset += 16;
        }
        out[offset..offset + 2].copy_from_slice(&(payload.len() as u16).to_le_bytes());
        offset += 2;
        out[offset..offset + payload.len()].copy_from_slice(payload);
        Ok(used)
    }

    /// Inverse of [`Self::encode`]; returns the frame view plus the payload
    /// slice it borrows.
    pub fn decode(bytes: &[u8]) -> Result<(EapolKeyFrame, &[u8]), WireError> {
        if bytes.len() < 9 {
            return Err(WireError::TooShort);
        }
        let kind = bytes[0];
        let replay = u64::from_be_bytes(bytes[1..9].try_into().expect("replay slice"));
        let mut offset = 9usize;
        let mut nonce = None;
        match kind {
            EAPOL_MESSAGE_1 | EAPOL_MESSAGE_2 => {
                if bytes.len() < offset + 32 {
                    return Err(WireError::TooShort);
                }
                let mut value = [0u8; 32];
                value.copy_from_slice(&bytes[offset..offset + 32]);
                nonce = Some(value);
                offset += 32;
            }
            EAPOL_MESSAGE_3 | EAPOL_MESSAGE_4 => {}
            _ => return Err(WireError::BadKind),
        }
        let mut mic = None;
        if kind >= EAPOL_MESSAGE_2 {
            if bytes.len() < offset + 16 {
                return Err(WireError::TooShort);
            }
            let mut value = [0u8; 16];
            value.copy_from_slice(&bytes[offset..offset + 16]);
            mic = Some(value);
            offset += 16;
        }
        if bytes.len() < offset + 2 {
            return Err(WireError::TooShort);
        }
        let payload_len = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]) as usize;
        offset += 2;
        if bytes.len() - offset != payload_len {
            return Err(WireError::BadPayloadLength);
        }
        let payload = &bytes[offset..offset + payload_len];
        Ok((
            EapolKeyFrame {
                kind,
                replay,
                nonce,
                mic,
                payload_len,
            },
            payload,
        ))
    }
}

// ---------------------------------------------------------------------------
// WPA2-PSK 4-way handshake — authenticator side (pure state machine)
// ---------------------------------------------------------------------------

/// Handshake failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandshakeError {
    /// Message received in the wrong state.
    WrongState,
    /// Message kind did not match the expected step.
    WrongMessageType,
    /// Replay counter did not match the issued one.
    ReplayMismatch,
    /// MIC slot did not verify against the derived KCK.
    MicMismatch,
}

/// Authenticator handshake phases.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandshakeState {
    /// Before message 1 is sent.
    AwaitingMessage1,
    /// Message 1 sent (PTK derived); awaiting the supplicant's message 2.
    AwaitingMessage2,
    /// Message 2 MIC verified; message 3 not yet emitted.
    Message2Verified,
    /// Message 3 sent; awaiting the supplicant's message 4.
    AwaitingMessage4,
    /// Message 4 verified; PTK installed.
    Installed,
}

/// WPA2-PSK authenticator: drives message1 → derive PTK → verify message2
/// MIC slot → emit message3 → verify message4.
///
/// The supplicant role is intentionally out of scope here (the kernel-side
/// station would need it; see roadmap rows 101/102).
pub struct Authenticator {
    pmk: [u8; 32],
    aa: [u8; 6],
    spa: [u8; 6],
    anonce: [u8; 32],
    snonce: [u8; 32],
    ptk: Option<Ptk>,
    replay_tx: u64,
    state: HandshakeState,
}

impl Authenticator {
    /// New authenticator for the AP address `aa` and station address `spa`.
    pub fn new(pmk: [u8; 32], aa: [u8; 6], spa: [u8; 6]) -> Authenticator {
        Authenticator {
            pmk,
            aa,
            spa,
            anonce: [0; 32],
            snonce: [0; 32],
            ptk: None,
            replay_tx: 0,
            state: HandshakeState::AwaitingMessage1,
        }
    }

    /// Current phase.
    pub fn state(&self) -> HandshakeState {
        self.state
    }

    /// Derived PTK once available (after message 1).
    pub fn ptk(&self) -> Option<&Ptk> {
        self.ptk.as_ref()
    }

    /// Sends message 1: latches ANonce, derives the SNonce (deterministic
    /// placeholder from the PMK and addresses) and the PTK.
    pub fn send_message1(&mut self, anonce: [u8; 32]) {
        self.anonce = anonce;
        // Deterministic SNonce placeholder (real hardware draws from an RNG):
        // HMAC over the role-binding inputs keeps the derivation testable.
        let mut snonce = [0u8; 32];
        let digest = hmac_sha512(&self.pmk, &[b"SNonce", &self.spa, &self.aa, &anonce]);
        snonce.copy_from_slice(&digest[..32]);
        self.snonce = snonce;
        self.ptk = Some(derive_ptk_placeholder(
            &self.pmk,
            &self.aa,
            &self.spa,
            &anonce,
            &self.snonce,
        ));
        self.replay_tx = 1;
        self.state = HandshakeState::AwaitingMessage2;
    }

    /// The SNonce this authenticator generated for message 2 cross-checks.
    pub fn snonce(&self) -> &[u8; 32] {
        &self.snonce
    }

    /// Verifies the supplicant's message 2 MIC slot and replay counter.
    /// `payload` is the key-data section the MIC covers.
    pub fn on_message2(
        &mut self,
        frame: &EapolKeyFrame,
        payload: &[u8],
    ) -> Result<(), HandshakeError> {
        if self.state != HandshakeState::AwaitingMessage2 {
            return Err(HandshakeError::WrongState);
        }
        if frame.kind != EAPOL_MESSAGE_2 {
            return Err(HandshakeError::WrongMessageType);
        }
        if frame.replay != self.replay_tx {
            return Err(HandshakeError::ReplayMismatch);
        }
        let mic = frame.mic.ok_or(HandshakeError::MicMismatch)?;
        if mic != self.mic_placeholder(frame.kind, frame.replay, payload) {
            return Err(HandshakeError::MicMismatch);
        }
        self.state = HandshakeState::Message2Verified;
        Ok(())
    }

    /// Emits message 3 after a verified message 2; advances to awaiting
    /// message 4. `payload` is the key data carried (and MICed) by the
    /// frame.
    pub fn send_message3(&mut self, payload: &[u8]) -> Result<EapolKeyFrame, HandshakeError> {
        if self.state != HandshakeState::Message2Verified {
            return Err(HandshakeError::WrongState);
        }
        self.replay_tx += 1;
        let mic = self.mic_placeholder(EAPOL_MESSAGE_3, self.replay_tx, payload);
        let frame = EapolKeyFrame {
            kind: EAPOL_MESSAGE_3,
            replay: self.replay_tx,
            nonce: None,
            mic: Some(mic),
            payload_len: payload.len(),
        };
        self.state = HandshakeState::AwaitingMessage4;
        Ok(frame)
    }

    /// Verifies the supplicant's message 4 and installs the PTK.
    pub fn on_message4(
        &mut self,
        frame: &EapolKeyFrame,
        payload: &[u8],
    ) -> Result<(), HandshakeError> {
        if self.state != HandshakeState::AwaitingMessage4 {
            return Err(HandshakeError::WrongState);
        }
        if frame.kind != EAPOL_MESSAGE_4 {
            return Err(HandshakeError::WrongMessageType);
        }
        if frame.replay != self.replay_tx {
            return Err(HandshakeError::ReplayMismatch);
        }
        let mic = frame.mic.ok_or(HandshakeError::MicMismatch)?;
        if mic != self.mic_placeholder(frame.kind, frame.replay, payload) {
            return Err(HandshakeError::MicMismatch);
        }
        self.state = HandshakeState::Installed;
        Ok(())
    }
}

impl Authenticator {
    /// Placeholder MIC over the coverage (kind byte, replay counter,
    /// payload) under the derived KCK.
    fn mic_placeholder(&self, kind: u8, replay: u64, payload: &[u8]) -> [u8; 16] {
        let kind_bytes = [kind];
        let replay_bytes = replay.to_be_bytes();
        let ptk = self.ptk.expect("PTK derived at message 1");
        eapol_mic_placeholder(&ptk.kck, &[&kind_bytes, &replay_bytes, payload])
    }
}

// ---------------------------------------------------------------------------
// Link-state machine
// ---------------------------------------------------------------------------

/// Link phases.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkState {
    Down,
    Scanning,
    Authenticating,
    Associating,
    Connected,
}

/// Link-driving events.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkEvent {
    ScanStarted,
    ScanComplete,
    JoinRequested,
    AuthOk,
    AssocOk,
    Deauth,
}

/// Illegal event for the current phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LinkStateError;

/// Down → Scanning → Authenticating → Associating → Connected, with
/// timeout (transient states) and deauth (any state) transitions back to
/// [`LinkState::Down`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LinkMonitor {
    state: Option<LinkState>,
}

impl LinkMonitor {
    /// Starts at [`LinkState::Down`].
    pub fn new() -> LinkMonitor {
        LinkMonitor {
            state: Some(LinkState::Down),
        }
    }

    /// Current phase.
    pub fn state(&self) -> LinkState {
        self.state.unwrap_or(LinkState::Down)
    }

    /// Applies one event; illegal transitions are rejected with the current
    /// state untouched.
    pub fn advance(&mut self, event: LinkEvent) -> Result<LinkState, LinkStateError> {
        let current = self.state();
        let next = match (current, event) {
            (LinkState::Down, LinkEvent::ScanStarted) => LinkState::Scanning,
            (LinkState::Scanning, LinkEvent::JoinRequested) => LinkState::Authenticating,
            (LinkState::Authenticating, LinkEvent::AuthOk) => LinkState::Associating,
            (LinkState::Associating, LinkEvent::AssocOk) => LinkState::Connected,
            (LinkState::Connected, LinkEvent::Deauth)
            | (LinkState::Associating, LinkEvent::Deauth)
            | (LinkState::Authenticating, LinkEvent::Deauth)
            | (LinkState::Scanning, LinkEvent::Deauth)
            | (LinkState::Down, LinkEvent::Deauth) => LinkState::Down,
            // Timeout handled separately; ScanComplete just ends the scan
            // window without forcing a state (targets may or may not exist).
            (LinkState::Scanning, LinkEvent::ScanComplete) => LinkState::Scanning,
            _ => return Err(LinkStateError),
        };
        self.state = Some(next);
        Ok(next)
    }

    /// Watchdog tick: any transient phase times out to
    /// [`LinkState::Down`]; Down/Connected are stable against ticks.
    pub fn on_timeout(&mut self) -> LinkState {
        match self.state() {
            LinkState::Scanning | LinkState::Authenticating | LinkState::Associating => {
                self.state = Some(LinkState::Down);
            }
            _ => {}
        }
        self.state()
    }
}
