//! Shared, dependency-free wireless wire primitives extracted from the
//! platform pure layer (`platform/x86_64/qemu_virtio/src/wireless.rs` is the
//! canonical source and re-exports these items so its own surface and test
//! harness are unchanged).
//!
//! Scope: exactly the pieces the service layer consumes —
//! - scan-record decode (`ScanEntry`, `decode_scan_record`, `Security`),
//! - saved-network store codec (`SavedNetwork`, `SavedNetworkStore`),
//! - link state machine names (`LinkState`, `LinkEvent`, `LinkMonitor`).
//!
//! The CFG80211-style envelope builder/parser, EAPOL key-frame codec and
//! 4-way-handshake authenticator stay in the platform module: they are
//! device-transport concerns, not service-layer wire shapes, and keep the
//! SHA-512 dependency over there.
//!
//! UNTESTED WITHOUT HARDWARE (inherited honesty note): these codecs have
//! never run against a real 802.11 NIC; see the platform module header.
#![no_std]
#![allow(dead_code)]

/// Maximum SSID octet length per 802.11.
pub const MAX_SSID_LEN: usize = 32;
/// Maximum PSK octet length (64 for raw passphrases).
pub const MAX_PSK_LEN: usize = 64;
/// Saved-network slots (no heap; store is fixed-capacity).
pub const MAX_SAVED_NETWORKS: usize = 8;

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
// Link state machine (Down → Scanning → Authenticating → Associating →
// Connected)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_record_decode_extracts_ssid_channel_rssi_bssid() {
        let mut body = [0u8; 45];
        // MAC header with fc type=management (00), addr3 = BSSID at [16..22].
        body[16] = 0x10;
        body[21] = 0x60;
        // Fixed fields are zero (timestamp/beacon-interval/capability).
        // IE: SSID "home"
        body[36] = 0;
        body[37] = 4;
        body[38..42].copy_from_slice(b"home");
        // IE: DS parameter set, channel 6
        body[42] = 3;
        body[43] = 1;
        body[44] = 6;
        let mut record = [0u8; 49];
        record[0] = 0xd8; // rssi = -40
        record[1] = 11;
        let body_len = body.len() as u16;
        record[2..4].copy_from_slice(&body_len.to_le_bytes());
        record[4..].copy_from_slice(&body);

        let entry = decode_scan_record(&record).expect("decodes");
        assert_eq!(entry.ssid, b"home");
        assert_eq!(entry.channel, 6);
        assert_eq!(entry.rssi, -40);
        assert_eq!(entry.bssid, [0x10, 0, 0, 0, 0, 0x60]);
        assert_eq!(entry.security, Security::Open);
    }

    #[test]
    fn scan_record_rsne_classifies_wpa2_wpa3_unknown() {
        let mut body = [0u8; 58];
        // RSNE with one pairwise suite, AKM suite type 2 (WPA2-PSK).
        let rsne: [u8; 20] = [
            0x01, 0x00, // version 1
            0x00, 0x0f, 0xac, 0x04, // group CCMP
            0x01, 0x00, // one pairwise suite
            0x00, 0x0f, 0xac, 0x04, 0x01, 0x00, // one AKM suite
            0x00, 0x0f, 0xac, 0x02, // PSK AKM
            0x00, 0x00, // RSN capabilities
        ];
        body[36] = 48;
        body[37] = rsne.len() as u8;
        body[38..38 + rsne.len()].copy_from_slice(&rsne);
        let mut record = [0u8; 62];
        let body_len = 58u16;
        record[2..4].copy_from_slice(&body_len.to_le_bytes());
        record[4..].copy_from_slice(&body);
        let entry = decode_scan_record(&record).expect("decodes");
        assert_eq!(entry.security, Security::Wpa2);

        // AKM suite type 8 → WPA3 (suite type is the selector's high byte).
        let mut wpa3 = rsne;
        wpa3[17] = 8;
        body[38..38 + rsne.len()].copy_from_slice(&wpa3);
        record[4..].copy_from_slice(&body);
        let entry = decode_scan_record(&record).expect("decodes");
        assert_eq!(entry.security, Security::Wpa3);

        // Bad version → Unknown.
        let mut bad = rsne;
        bad[0] = 9;
        body[38..38 + rsne.len()].copy_from_slice(&bad);
        record[4..].copy_from_slice(&body);
        let entry = decode_scan_record(&record).expect("decodes");
        assert_eq!(entry.security, Security::Unknown);
    }

    #[test]
    fn scan_record_rejects_truncated_and_non_mgmt() {
        assert_eq!(decode_scan_record(&[0; 3]), Err(DecodeError::TooShort));
        // Declared body longer than present.
        let mut record = [0u8; 10];
        record[2..4].copy_from_slice(&100u16.to_le_bytes());
        assert_eq!(decode_scan_record(&record), Err(DecodeError::BadBodyLength));
        // Data frame (fc type bits nonzero).
        let mut body = [0u8; 36];
        body[0] = 0x08; // fc type = data
        let mut record = [0u8; 40];
        record[2..4].copy_from_slice(&36u16.to_le_bytes());
        record[4..].copy_from_slice(&body);
        assert_eq!(
            decode_scan_record(&record),
            Err(DecodeError::NotManagementFrame)
        );
    }

    #[test]
    fn saved_store_roundtrips_through_codec() {
        let mut store = SavedNetworkStore::new();
        store
            .insert(SavedNetwork::new(b"home", b"passphrase1", None, 3).expect("record"))
            .expect("insert");
        store
            .insert(
                SavedNetwork::new(b"office", b"other-passphrase", Some([1, 2, 3, 4, 5, 6]), 9)
                    .expect("record"),
            )
            .expect("insert");

        let mut buffer = [0u8; 256];
        let used = store.encode(&mut buffer).expect("encodes");
        let decoded = SavedNetworkStore::decode(&buffer[..used]).expect("decodes");
        assert_eq!(decoded, store);
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded.best().expect("best").ssid_bytes(), b"office");

        // Bad magic and truncated headers are rejected.
        assert_eq!(
            SavedNetworkStore::decode(&[0; 4]),
            Err(CodecError::BadMagic)
        );
        assert_eq!(
            SavedNetworkStore::decode(&buffer[..3]),
            Err(CodecError::BadRecord)
        );
    }

    #[test]
    fn saved_store_replaces_same_ssid_and_enforces_capacity() {
        let mut store = SavedNetworkStore::new();
        for index in 0..MAX_SAVED_NETWORKS {
            let ssid = [b'a' + index as u8; 4];
            store
                .insert(SavedNetwork::new(&ssid, b"psk-value", None, 0).expect("record"))
                .expect("capacity holds");
        }
        // One more distinct SSID is rejected.
        assert!(
            store
                .insert(SavedNetwork::new(b"overflow", b"psk-value", None, 0).expect("record"))
                .is_none()
        );
        // Same-SSID insert replaces in place.
        store
            .insert(SavedNetwork::new(&[b'a'; 4], b"new-psk!!", None, 1).expect("record"))
            .expect("replace");
        assert_eq!(store.len(), MAX_SAVED_NETWORKS);
        // Removal compacts and frees a slot.
        assert!(store.remove(&[b'a'; 4]));
        assert_eq!(store.len(), MAX_SAVED_NETWORKS - 1);
        assert!(!store.remove(b"missing"));
    }

    #[test]
    fn link_monitor_walks_join_path_and_rejects_illegal_events() {
        let mut link = LinkMonitor::new();
        assert_eq!(link.state(), LinkState::Down);
        assert!(link.advance(LinkEvent::JoinRequested).is_err());
        assert_eq!(
            link.advance(LinkEvent::ScanStarted),
            Ok(LinkState::Scanning)
        );
        assert_eq!(
            link.advance(LinkEvent::JoinRequested),
            Ok(LinkState::Authenticating)
        );
        assert_eq!(link.advance(LinkEvent::AuthOk), Ok(LinkState::Associating));
        assert_eq!(link.advance(LinkEvent::AssocOk), Ok(LinkState::Connected));
        assert_eq!(link.advance(LinkEvent::ScanStarted).is_err(), true);
        assert_eq!(link.advance(LinkEvent::Deauth), Ok(LinkState::Down));
    }
}
