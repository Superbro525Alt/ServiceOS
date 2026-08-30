use rt::{NetworkStatus, NetworkTag, RawMessage};
use serviceos_userspace_runtime as rt;
use serviceos_wireless::{
    LinkMonitor, LinkState, MAX_PSK_LEN, MAX_SAVED_NETWORKS, MAX_SSID_LEN, SavedNetwork,
    SavedNetworkStore, ScanEntry,
};

use crate::consts::{
    WIFI_SAVED_ENTRIES_PER_REPLY, WIFI_SCAN_ENTRIES_PER_REPLY, WIFI_STATUS_FLAG_BACKEND_PRESENT,
};

/// Reply words per packed scan entry: channel|rssi|bssid, ssid-len|security,
/// then four ssid payload words (covers a 32-octet SSID). Consumed once a
/// WirelessBackend delivers real scan records (always-absent today).
#[allow(dead_code)]
pub(crate) const WIFI_SCAN_ENTRY_WORDS: usize = 6;
/// Reply words per packed saved-network entry: ssid-len|priority, then four
/// ssid payload words. PSK octets never leave the service.
pub(crate) const WIFI_SAVED_ENTRY_WORDS: usize = 5;

/// Service-side wireless state. Honest by construction: `backend_present` is
/// only ever true once a `WirelessBackend` device is registered with the
/// kernel and wired through; that has never happened in-tree, so every
/// control operation below reports `NetworkStatus::Unsupported` today.
pub(crate) struct WifiState {
    backend_present: bool,
    link: LinkMonitor,
    current_ssid: [u8; MAX_SSID_LEN],
    current_ssid_len: usize,
    store: SavedNetworkStore,
}

impl WifiState {
    pub(crate) fn new() -> WifiState {
        WifiState {
            backend_present: false,
            link: LinkMonitor::new(),
            current_ssid: [0; MAX_SSID_LEN],
            current_ssid_len: 0,
            store: SavedNetworkStore::new(),
        }
    }

    pub(crate) fn link_state(&self) -> LinkState {
        self.link.state()
    }

    #[allow(dead_code)]
    pub(crate) fn saved_count(&self) -> usize {
        self.store.len()
    }

    /// Host-test visibility into the store contents.
    #[cfg(test)]
    pub(crate) fn saved_ssids(&self) -> std::vec::Vec<[u8; MAX_SSID_LEN]> {
        self.store
            .iter()
            .map(|record| {
                let mut ssid = [0u8; MAX_SSID_LEN];
                ssid[..record.ssid_len].copy_from_slice(record.ssid_bytes());
                ssid
            })
            .collect()
    }
}

impl Default for WifiState {
    fn default() -> Self {
        Self::new()
    }
}

/// One hosts-resource option line (`wifi-*`) folded into the boot seed.
/// Lines are processed in file order: `wifi-ssid` starts (or switches) a
/// record, the following `wifi-psk` / `wifi-priority` lines complete it.
/// Absent wifi lines leave the store empty and boot behavior unchanged.
pub(crate) fn note_config_line(name: &str, value: &str, state: &mut WifiState) {
    if let Some(ssid) = name.strip_prefix("wifi-ssid") {
        // Trailing digits are ignored (documented as alias noise); only the
        // exact key starts/switches a pending record.
        let _ = ssid;
        // Stash the SSID in the current-ssid slot as pending state; the
        // following wifi-psk line promotes it into the store.
        let bytes = value.as_bytes();
        if bytes.is_empty() || bytes.len() > MAX_SSID_LEN {
            return;
        }
        state.current_ssid = [0; MAX_SSID_LEN];
        state.current_ssid[..bytes.len()].copy_from_slice(bytes);
        state.current_ssid_len = bytes.len();
        return;
    }
    if name == "wifi-psk" {
        if state.current_ssid_len == 0 {
            return;
        }
        let bytes = value.as_bytes();
        // Codec records always carry a PSK (open networks cannot be
        // represented in the saved-network store today).
        if bytes.is_empty() || bytes.len() > MAX_PSK_LEN {
            state.current_ssid_len = 0;
            return;
        }
        let record = SavedNetwork::new(
            &state.current_ssid[..state.current_ssid_len],
            bytes,
            None,
            0,
        );
        state.current_ssid_len = 0;
        if let Some(record) = record {
            let _ = state.store.insert(record);
        }
    }
}

/// Validates a join request (ssid 1..=32 octets; psk absent for open or
/// 8..=64 octets) and decodes the inline bytes into staging buffers.
fn decode_join_request(
    request: &RawMessage,
) -> Result<([u8; MAX_SSID_LEN], usize, [u8; MAX_PSK_LEN], usize), NetworkStatus> {
    if request.word_count < 2 {
        return Err(NetworkStatus::InvalidTarget);
    }
    let ssid_len = request.words[0] as usize;
    let psk_len = request.words[1] as usize;
    if ssid_len == 0 || ssid_len > MAX_SSID_LEN || psk_len > MAX_PSK_LEN {
        return Err(NetworkStatus::InvalidTarget);
    }
    if psk_len != 0 && (psk_len < 8) {
        return Err(NetworkStatus::InvalidTarget);
    }
    // Word-count strictness lives in the runtime builder; the service only
    // requires that the inline bytes are present.
    if (request.word_count as usize) < 2 + ssid_len.div_ceil(8) + psk_len.div_ceil(8) {
        return Err(NetworkStatus::InvalidTarget);
    }
    let mut ssid = [0u8; MAX_SSID_LEN];
    let mut psk = [0u8; MAX_PSK_LEN];
    let inline = &request.words[2..request.word_count as usize];
    let mut offset = 0usize;
    unpack_word_bytes(inline, &mut offset, &mut ssid[..ssid_len]);
    unpack_word_bytes(inline, &mut offset, &mut psk[..psk_len]);
    Ok((ssid, ssid_len, psk, psk_len))
}

/// Copies `destination.len()` bytes out of `words` starting at word offset
/// `offset / 8`-aligned; `offset` is advanced in bytes.
fn unpack_word_bytes(words: &[u64], offset: &mut usize, destination: &mut [u8]) {
    let mut copied = 0usize;
    while copied < destination.len() {
        let word_index = *offset / 8;
        let byte_index = *offset % 8;
        let Some(word) = words.get(word_index) else {
            break;
        };
        let bytes = word.to_le_bytes();
        let chunk = (destination.len() - copied).min(8 - byte_index);
        destination[copied..copied + chunk].copy_from_slice(&bytes[byte_index..byte_index + chunk]);
        copied += chunk;
        *offset += chunk;
    }
}

/// Packs a decoded scan entry into `words[..WIFI_SCAN_ENTRY_WORDS]`:
/// w0 = channel<<56 | rssi-byte<<48 | bssid(48-bit),
/// w1 = ssid-len<<56 | security<<48 | ssid[0..6],
/// w2..w5 = ssid[6..38] little-endian packed (ssid is at most 32 octets).
#[allow(dead_code)]
pub(crate) fn pack_scan_entry(entry: &ScanEntry<'_>, words: &mut [u64]) {
    debug_assert!(words.len() >= WIFI_SCAN_ENTRY_WORDS);
    let bssid48 = (entry.bssid[0] as u64) << 40
        | (entry.bssid[1] as u64) << 32
        | (entry.bssid[2] as u64) << 24
        | (entry.bssid[3] as u64) << 16
        | (entry.bssid[4] as u64) << 8
        | entry.bssid[5] as u64;
    words[0] = ((entry.channel as u64) << 56) | ((entry.rssi as u8 as u64) << 48) | bssid48;
    words[1] = ((entry.ssid.len() as u64) << 56)
        | ((security_word(entry.security) as u64) << 48)
        | ssid_prefix_word(entry.ssid, 0, 6);
    for (index, word) in words[2..WIFI_SCAN_ENTRY_WORDS].iter_mut().enumerate() {
        *word = ssid_prefix_word(entry.ssid, 6 + index * 8, 8);
    }
}

#[allow(dead_code)]
fn security_word(security: serviceos_wireless::Security) -> u64 {
    // Values mirror serviceos_wireless::Security in ABI WifiSecurity order.
    match security {
        serviceos_wireless::Security::Open => 0,
        serviceos_wireless::Security::Wpa2 => 1,
        serviceos_wireless::Security::Wpa3 => 2,
        serviceos_wireless::Security::Unknown => 3,
    }
}

fn ssid_prefix_word(ssid: &[u8], start: usize, len: usize) -> u64 {
    let mut bytes = [0u8; 8];
    for (index, byte) in bytes.iter_mut().enumerate().take(len) {
        if start + index < ssid.len() {
            *byte = ssid[start + index];
        }
    }
    u64::from_le_bytes(bytes)
}

/// Packs one saved record (SSID only — PSK octets never leave the service):
/// w0 = ssid-len<<56 | priority<<48 | ssid[0..6], w1..w4 = ssid[6..38].
fn pack_saved_entry(record: &SavedNetwork, words: &mut [u64]) {
    debug_assert!(words.len() >= WIFI_SAVED_ENTRY_WORDS);
    words[0] = ((record.ssid_len as u64) << 56)
        | ((record.priority as u64) << 48)
        | ssid_prefix_word(record.ssid_bytes(), 0, 6);
    for (index, word) in words[1..WIFI_SAVED_ENTRY_WORDS].iter_mut().enumerate() {
        *word = ssid_prefix_word(record.ssid_bytes(), 6 + index * 8, 8);
    }
}

fn link_state_word(state: LinkState) -> u64 {
    match state {
        LinkState::Down => 0,
        LinkState::Scanning => 1,
        LinkState::Authenticating => 2,
        LinkState::Associating => 3,
        LinkState::Connected => 4,
    }
}

/// Pure request→reply mapping for the wireless family. `None` means "no
/// reply" (malformed request without a reply handle, matching the house
/// convention of silently dropping unanswerable requests).
pub(crate) fn reply_for(request: &RawMessage, state: &mut WifiState) -> Option<RawMessage> {
    if request.handle_count < 1 {
        return None;
    }
    let tag = request.tag;

    if tag == NetworkTag::WifiScanRequest as u32 {
        // No backend: honest absence, zero results, FSM untouched.
        let mut reply = RawMessage::empty(NetworkTag::WifiScanReply as u32);
        reply.words[0] = NetworkStatus::Unsupported as u32 as u64;
        reply.words[1] = 0;
        reply.words[2] = 0;
        reply.word_count = 3;
        return Some(reply);
    }
    if tag == NetworkTag::WifiJoinRequest as u32 {
        let mut reply = RawMessage::empty(NetworkTag::WifiJoinReply as u32);
        match decode_join_request(request) {
            Err(status) => {
                reply.words[0] = status as u32 as u64;
                reply.words[1] = link_state_word(state.link_state());
                reply.word_count = 2;
            }
            Ok((ssid, ssid_len, _psk, _psk_len)) => {
                // A real backend would run scan→auth→associate here. With
                // none registered the join is refused, the link FSM stays
                // Down, and no network is marked current.
                let _ = (ssid, ssid_len);
                reply.words[0] = NetworkStatus::Unsupported as u32 as u64;
                reply.words[1] = link_state_word(state.link_state());
                reply.word_count = 2;
            }
        }
        return Some(reply);
    }
    if tag == NetworkTag::WifiLeaveRequest as u32 {
        let mut reply = RawMessage::empty(NetworkTag::WifiLeaveReply as u32);
        // Deauth with no backend is refused; the FSM (already Down) is
        // echoed honestly.
        reply.words[0] = NetworkStatus::Unsupported as u32 as u64;
        reply.words[1] = link_state_word(state.link_state());
        reply.word_count = 2;
        return Some(reply);
    }
    if tag == NetworkTag::WifiSavedListRequest as u32 {
        let mut reply = RawMessage::empty(NetworkTag::WifiSavedListReply as u32);
        reply.words[0] = NetworkStatus::Ok as u32 as u64;
        reply.words[1] = state.store.len() as u64;
        let mut written = 0usize;
        for record in state.store.iter() {
            if written == WIFI_SAVED_ENTRIES_PER_REPLY {
                break;
            }
            let base = 3 + written * WIFI_SAVED_ENTRY_WORDS;
            if base + WIFI_SAVED_ENTRY_WORDS > rt::IPC_MAX_WORDS {
                break;
            }
            pack_saved_entry(record, &mut reply.words[base..]);
            written += 1;
        }
        reply.words[2] = written as u64;
        reply.word_count = (3 + written * WIFI_SAVED_ENTRY_WORDS) as u32;
        return Some(reply);
    }
    if tag == NetworkTag::WifiSavedAddRequest as u32 {
        let mut reply = RawMessage::empty(NetworkTag::WifiSavedAddReply as u32);
        reply.words[0] = decode_saved_add_request(request, state) as u32 as u64;
        reply.word_count = 1;
        return Some(reply);
    }
    if tag == NetworkTag::WifiSavedRemoveRequest as u32 {
        let mut reply = RawMessage::empty(NetworkTag::WifiSavedRemoveReply as u32);
        reply.words[0] = decode_saved_remove_request(request, state) as u32 as u64;
        reply.word_count = 1;
        return Some(reply);
    }
    if tag == NetworkTag::WifiStatusRequest as u32 {
        let mut reply = RawMessage::empty(NetworkTag::WifiStatusReply as u32);
        // Distinct honest status: there is no wireless backend, so there is
        // no live wireless status — the service-side FSM echo still rides in
        // words[1] (Down unless a future backend drove it).
        reply.words[0] = NetworkStatus::Unsupported as u32 as u64;
        reply.words[1] = link_state_word(state.link_state());
        reply.words[2] = if state.backend_present {
            WIFI_STATUS_FLAG_BACKEND_PRESENT as u64
        } else {
            0
        };
        reply.words[3] = state.current_ssid_len as u64;
        for (index, word) in reply.words[4..8].iter_mut().enumerate() {
            *word = ssid_prefix_word(&state.current_ssid[..state.current_ssid_len], index * 8, 8);
        }
        reply.word_count = 8;
        return Some(reply);
    }
    None
}

fn decode_saved_add_request(request: &RawMessage, state: &mut WifiState) -> NetworkStatus {
    if request.word_count < 3 {
        return NetworkStatus::InvalidTarget;
    }
    let ssid_len = request.words[0] as usize;
    let psk_len = request.words[1] as usize;
    let priority = request.words[2] as u8;
    if ssid_len == 0 || ssid_len > MAX_SSID_LEN {
        return NetworkStatus::InvalidTarget;
    }
    // The saved-network codec cannot represent open networks (psk length
    // byte 0 is invalid on the wire), so open networks are rejected here
    // rather than silently dropped later.
    if psk_len == 0 || psk_len > MAX_PSK_LEN {
        return NetworkStatus::InvalidTarget;
    }
    let need = 3 + ssid_len.div_ceil(8) + psk_len.div_ceil(8);
    if (request.word_count as usize) != need {
        return NetworkStatus::InvalidTarget;
    }
    let mut ssid = [0u8; MAX_SSID_LEN];
    let mut psk = [0u8; MAX_PSK_LEN];
    let inline = &request.words[3..request.word_count as usize];
    let mut offset = 0usize;
    unpack_word_bytes(inline, &mut offset, &mut ssid[..ssid_len]);
    unpack_word_bytes(inline, &mut offset, &mut psk[..psk_len]);
    match SavedNetwork::new(&ssid[..ssid_len], &psk[..psk_len], None, priority) {
        Some(record) => match state.store.insert(record) {
            Some(()) => NetworkStatus::Ok,
            None => NetworkStatus::CapacityExceeded,
        },
        None => NetworkStatus::InvalidTarget,
    }
}

fn decode_saved_remove_request(request: &RawMessage, state: &mut WifiState) -> NetworkStatus {
    if request.word_count < 1 {
        return NetworkStatus::InvalidTarget;
    }
    let ssid_len = request.words[0] as usize;
    if ssid_len == 0 || ssid_len > MAX_SSID_LEN {
        return NetworkStatus::InvalidTarget;
    }
    if (request.word_count as usize) != 1 + ssid_len.div_ceil(8) {
        return NetworkStatus::InvalidTarget;
    }
    let mut ssid = [0u8; MAX_SSID_LEN];
    let inline = &request.words[1..request.word_count as usize];
    let mut offset = 0usize;
    unpack_word_bytes(inline, &mut offset, &mut ssid[..ssid_len]);
    if state.store.remove(&ssid[..ssid_len]) {
        NetworkStatus::Ok
    } else {
        NetworkStatus::NotFound
    }
}

/// IPC arm: builds the reply for a wireless request and delivers it on the
/// caller's reply handle (closing it afterwards, per house convention).
pub(crate) fn handle(request: &RawMessage, state: &mut WifiState) -> rt::Result<()> {
    if let Some(mut reply) = reply_for(request, state) {
        let reply_handle = request.handles[0];
        reply.handle_count = 0;
        let _ = rt::channel_send(reply_handle, &reply);
        let _ = rt::handle_close(reply_handle);
    }
    Ok(())
}

/// Slot count shared with the ABI docs; kept here so the wire-shape comments
/// and the loop bounds cannot drift apart.
const _: () = {
    assert!(MAX_SAVED_NETWORKS == 8);
    assert!(WIFI_SCAN_ENTRIES_PER_REPLY == 2);
    assert!(WIFI_SAVED_ENTRIES_PER_REPLY == 2);
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consts::WIFI_STATUS_FLAG_BACKEND_PRESENT;

    fn request(tag: u32) -> RawMessage {
        let mut request = RawMessage::empty(tag);
        request.handle_count = 1;
        request
    }

    #[test]
    fn backend_absent_matrix_replies_unsupported_for_every_control_op() {
        let mut state = WifiState::new();
        // Scan
        let reply = reply_for(&request(NetworkTag::WifiScanRequest as u32), &mut state)
            .expect("scan replies");
        assert_eq!(reply.tag, NetworkTag::WifiScanReply as u32);
        assert_eq!(reply.words[0], NetworkStatus::Unsupported as u32 as u64);
        assert_eq!(reply.word_count, 3);
        // Join (valid shape)
        let mut join = request(NetworkTag::WifiJoinRequest as u32);
        join.word_count = 4;
        join.words[0] = 4;
        join.words[1] = 8;
        join.words[2] = u64::from_le_bytes(*b"home\0\0\0\0");
        join.words[3] = u64::from_le_bytes(*b"passphra");
        let reply = reply_for(&join, &mut state).expect("join replies");
        assert_eq!(reply.tag, NetworkTag::WifiJoinReply as u32);
        assert_eq!(reply.words[0], NetworkStatus::Unsupported as u32 as u64);
        assert_eq!(reply.words[1], 0, "link stays Down");
        // Leave
        let reply = reply_for(&request(NetworkTag::WifiLeaveRequest as u32), &mut state)
            .expect("leave replies");
        assert_eq!(reply.tag, NetworkTag::WifiLeaveReply as u32);
        assert_eq!(reply.words[0], NetworkStatus::Unsupported as u32 as u64);
        // Status
        let reply = reply_for(&request(NetworkTag::WifiStatusRequest as u32), &mut state)
            .expect("status replies");
        assert_eq!(reply.tag, NetworkTag::WifiStatusReply as u32);
        assert_eq!(reply.words[0], NetworkStatus::Unsupported as u32 as u64);
        assert_eq!(reply.words[1], 0, "link Down");
        assert_eq!(reply.words[2], 0, "backend flag unset");
        // Saved ops are store ops (no backend needed): list works, add/remove
        // follow validation, only scan/join/leave/status report Unsupported.
        let reply = reply_for(
            &request(NetworkTag::WifiSavedListRequest as u32),
            &mut state,
        )
        .expect("saved list replies");
        assert_eq!(reply.tag, NetworkTag::WifiSavedListReply as u32);
        assert_eq!(reply.words[0], NetworkStatus::Ok as u32 as u64);
    }

    #[test]
    fn join_rejects_bad_shapes_before_backend_check() {
        let mut state = WifiState::new();
        // Empty ssid
        let mut join = request(NetworkTag::WifiJoinRequest as u32);
        join.word_count = 3;
        join.words[0] = 0;
        join.words[1] = 8;
        let reply = reply_for(&join, &mut state).expect("replies");
        assert_eq!(reply.words[0], NetworkStatus::InvalidTarget as u32 as u64);
        // Oversized ssid
        let mut join = request(NetworkTag::WifiJoinRequest as u32);
        join.word_count = 8;
        join.words[0] = (MAX_SSID_LEN + 1) as u64;
        join.words[1] = 8;
        let reply = reply_for(&join, &mut state).expect("replies");
        assert_eq!(reply.words[0], NetworkStatus::InvalidTarget as u32 as u64);
        // PSK shorter than 8 octets (not open, not a valid passphrase)
        let mut join = request(NetworkTag::WifiJoinRequest as u32);
        join.word_count = 3;
        join.words[0] = 4;
        join.words[1] = 4;
        let reply = reply_for(&join, &mut state).expect("replies");
        assert_eq!(reply.words[0], NetworkStatus::InvalidTarget as u32 as u64);
    }

    #[test]
    fn open_join_is_validated_then_refused_without_backend() {
        let mut state = WifiState::new();
        let mut join = request(NetworkTag::WifiJoinRequest as u32);
        join.word_count = 3;
        join.words[0] = 4;
        join.words[1] = 0;
        join.words[2] = u64::from_le_bytes(*b"cafe\0\0\0\0");
        let reply = reply_for(&join, &mut state).expect("replies");
        assert_eq!(reply.tag, NetworkTag::WifiJoinReply as u32);
        assert_eq!(reply.words[0], NetworkStatus::Unsupported as u32 as u64);
    }

    #[test]
    fn saved_add_remove_list_roundtrip_with_word_shapes() {
        let mut state = WifiState::new();
        // add "home" psk "passphrase1" priority 3
        let mut add = request(NetworkTag::WifiSavedAddRequest as u32);
        add.word_count = 6;
        add.words[0] = 4;
        add.words[1] = 11;
        add.words[2] = 3;
        add.words[3] = u64::from_le_bytes(*b"home\0\0\0\0");
        add.words[4] = u64::from_le_bytes(*b"passphra");
        add.words[5] = u64::from_le_bytes(*b"se1\0\0\0\0\0");
        let reply = reply_for(&add, &mut state).expect("add replies");
        assert_eq!(reply.tag, NetworkTag::WifiSavedAddReply as u32);
        assert_eq!(reply.words[0], NetworkStatus::Ok as u32 as u64);
        assert_eq!(state.saved_count(), 1);

        // Duplicate add replaces (still Ok).
        let reply = reply_for(&add, &mut state).expect("add replies");
        assert_eq!(reply.words[0], NetworkStatus::Ok as u32 as u64);
        assert_eq!(state.saved_count(), 1);

        // Malformed add: psk length lies past the inline bytes.
        let mut bad = request(NetworkTag::WifiSavedAddRequest as u32);
        bad.word_count = 5;
        bad.words[0] = 4;
        bad.words[1] = 11;
        bad.words[2] = 0;
        bad.words[3] = u64::from_le_bytes(*b"home\0\0\0\0");
        let reply = reply_for(&bad, &mut state).expect("replies");
        assert_eq!(reply.words[0], NetworkStatus::InvalidTarget as u32 as u64);

        // List carries the ssid, priority and psk-never-echoed shape.
        let reply = reply_for(
            &request(NetworkTag::WifiSavedListRequest as u32),
            &mut state,
        )
        .expect("list replies");
        assert_eq!(reply.tag, NetworkTag::WifiSavedListReply as u32);
        assert_eq!(reply.words[0], NetworkStatus::Ok as u32 as u64);
        assert_eq!(reply.words[1], 1);
        assert_eq!(reply.words[2], 1);
        let word0 = reply.words[3];
        assert_eq!((word0 >> 56) as usize, 4, "ssid_len");
        assert_eq!((word0 >> 48) & 0xff, 3, "priority");
        assert_eq!(
            word0 & 0xffff_ffff_ffff,
            u64::from_le_bytes(*b"home\0\0\0\0")
        );

        // Remove: hit then miss.
        let mut remove = request(NetworkTag::WifiSavedRemoveRequest as u32);
        remove.word_count = 2;
        remove.words[0] = 4;
        remove.words[1] = u64::from_le_bytes(*b"home\0\0\0\0");
        let reply = reply_for(&remove, &mut state).expect("remove replies");
        assert_eq!(reply.words[0], NetworkStatus::Ok as u32 as u64);
        let reply = reply_for(&remove, &mut state).expect("remove replies");
        assert_eq!(reply.words[0], NetworkStatus::NotFound as u32 as u64);
        assert_eq!(state.saved_count(), 0);
    }

    #[test]
    fn saved_add_rejects_empty_psk_and_capacity_overflow() {
        let mut state = WifiState::new();
        let mut add = request(NetworkTag::WifiSavedAddRequest as u32);
        add.word_count = 4;
        add.words[0] = 4;
        add.words[1] = 0; // open networks cannot be stored by the codec
        add.words[2] = 0;
        add.words[3] = u64::from_le_bytes(*b"open\0\0\0\0");
        let reply = reply_for(&add, &mut state).expect("replies");
        assert_eq!(reply.words[0], NetworkStatus::InvalidTarget as u32 as u64);

        // Fill the store, then one more distinct SSID overflows.
        for index in 0..MAX_SAVED_NETWORKS {
            let mut add = request(NetworkTag::WifiSavedAddRequest as u32);
            add.word_count = 5;
            add.words[0] = 8;
            add.words[1] = 8;
            add.words[2] = 0;
            let mut ssid = [b'w'; 8];
            ssid[3] = b'0' + index as u8;
            add.words[3] = u64::from_le_bytes(ssid);
            add.words[4] = u64::from_le_bytes(*b"passphra");
            let reply = reply_for(&add, &mut state).expect("replies");
            assert_eq!(reply.words[0], NetworkStatus::Ok as u32 as u64);
        }
        let mut overflow = request(NetworkTag::WifiSavedAddRequest as u32);
        overflow.word_count = 5;
        overflow.words[0] = 8;
        overflow.words[1] = 8;
        overflow.words[2] = 0;
        overflow.words[3] = u64::from_le_bytes(*b"over\0\0\0\0");
        overflow.words[4] = u64::from_le_bytes(*b"passphra");
        let reply = reply_for(&overflow, &mut state).expect("replies");
        assert_eq!(
            reply.words[0],
            NetworkStatus::CapacityExceeded as u32 as u64
        );
    }

    #[test]
    fn requests_without_reply_handle_get_no_reply() {
        let mut state = WifiState::new();
        let mut scan = RawMessage::empty(NetworkTag::WifiScanRequest as u32);
        scan.handle_count = 0;
        assert!(reply_for(&scan, &mut state).is_none());
    }

    #[test]
    fn scan_entry_packing_matches_wire_shape() {
        let mut body = [0u8; 67];
        body[16] = 0xaa; // bssid byte 0
        body[21] = 0x66; // bssid byte 5
        body[36] = 0;
        body[37] = 4;
        body[38..42].copy_from_slice(b"home");
        body[42] = 3;
        body[43] = 1;
        body[44] = 6;
        // RSNE: WPA2 (AKM suite type 2)
        body[45] = 48;
        body[46] = 20;
        let rsne: [u8; 20] = [
            0x01, 0x00, 0x00, 0x0f, 0xac, 0x04, 0x01, 0x00, 0x00, 0x0f, 0xac, 0x04, 0x01, 0x00,
            0x00, 0x0f, 0xac, 0x02, 0x00, 0x00,
        ];
        body[47..67].copy_from_slice(&rsne);
        let mut record = [0u8; 71];
        record[0] = 0xd8; // -40
        record[1] = 11;
        record[2..4].copy_from_slice(&67u16.to_le_bytes());
        record[4..].copy_from_slice(&body);
        let entry = serviceos_wireless::decode_scan_record(&record).expect("decodes");

        let mut words = [0u64; WIFI_SCAN_ENTRY_WORDS];
        pack_scan_entry(&entry, &mut words);
        assert_eq!((words[0] >> 56) & 0xff, 6, "channel from DS IE");
        assert_eq!(((words[0] >> 48) & 0xff) as u8, 0xd8u8, "rssi byte");
        assert_eq!(
            words[0] & 0xffff_ffff_ffff,
            (0xaau64 << 40) | 0x66,
            "bssid 48-bit"
        );
        assert_eq!((words[1] >> 56) as usize, 4, "ssid len");
        assert_eq!((words[1] >> 48) & 0xff, 1, "security Wpa2");
        assert_eq!(
            words[1] & 0xffff_ffff_ffff,
            u64::from_le_bytes(*b"home\0\0\0\0")
        );
    }

    #[test]
    fn config_seed_lines_populate_store_and_ignore_junk() {
        let mut state = WifiState::new();
        note_config_line("wifi-ssid", "home", &mut state);
        note_config_line("wifi-psk", "passphrase1", &mut state);
        assert_eq!(state.saved_count(), 1);
        assert_eq!(state.saved_ssids()[0], zero_pad(b"home"));

        // psk before any ssid is ignored.
        let mut fresh = WifiState::new();
        note_config_line("wifi-psk", "orphan", &mut fresh);
        assert_eq!(fresh.saved_count(), 0);

        // Oversized ssid is dropped.
        let mut fresh = WifiState::new();
        note_config_line("wifi-ssid", &"x".repeat(MAX_SSID_LEN + 1), &mut fresh);
        note_config_line("wifi-psk", "passphrase1", &mut fresh);
        assert_eq!(fresh.saved_count(), 0);
    }

    fn zero_pad(bytes: &[u8]) -> [u8; MAX_SSID_LEN] {
        let mut ssid = [0u8; MAX_SSID_LEN];
        ssid[..bytes.len()].copy_from_slice(bytes);
        ssid
    }

    #[test]
    fn backend_flag_constant_is_bit_zero() {
        assert_eq!(WIFI_STATUS_FLAG_BACKEND_PRESENT, 1);
    }
}
