//! Protocol-conformance harness for the Wi-Fi pure protocol layer.
//!
//! Lives outside the lib test harness because `serviceos-platform-qemu-virtio`
//! sets `[lib] test = false` (its kernel-image dependency graph is not host
//! safe). This target includes the production `wireless.rs` by path — it is
//! pure and dependency-light (`serviceos-crypto` only) — so the real
//! envelope builder/parser, scan-record decoder, saved-network codec,
//! HMAC/PRF placeholders, 4-way-handshake authenticator and link state
//! machine all run on the host under std's default allocator.
//!
//! Crypto validation basis: RFC 4231 golden vectors prove the HMAC-SHA-512
//! primitive; the PTK/MIC placeholders above it are integrity-grade shapes
//! (see module header honesty notes) and are validated for structure,
//! determinism and sensitivity only.

#[path = "../src/wireless.rs"]
mod wireless;

use wireless::{
    Authenticator, CMD_ASSOCIATE, CMD_AUTHENTICATE, CMD_DISCONNECT, CMD_JOIN, CMD_TRIGGER_SCAN,
    CodecError, CommandBuilder, DecodeError, EapolKeyFrame, HandshakeError, HandshakeState,
    LinkEvent, LinkMonitor, LinkState, LinkStateError, MAX_SAVED_NETWORKS, ParseError, STATUS_OK,
    STATUS_REJECTED, SavedNetwork, SavedNetworkStore, ScanEntry, Security, WireError,
};

// ---------------------------------------------------------------------------
// Command envelope builder / parser
// ---------------------------------------------------------------------------

#[test]
fn command_builder_emits_golden_join_envelope() {
    let mut builder = CommandBuilder::new(CMD_JOIN, 7);
    builder.ssid(b"home").expect("ssid fits");
    builder
        .bssid(&[0x10, 0x20, 0x30, 0x40, 0x50, 0x60])
        .expect("bssid fits");
    builder.channel(6).expect("channel fits");
    let expected: &[u8] = &[
        0x03, 0x00, 0x07, 0x00, // cmd=JOIN, seq=7
        0x01, 0x04, b'h', b'o', b'm', b'e', // SSID attr
        0x02, 0x06, 0x10, 0x20, 0x30, 0x40, 0x50, 0x60, // BSSID attr
        0x03, 0x01, 0x06, // CHANNEL attr
    ];
    assert_eq!(builder.finish(), expected);
}

#[test]
fn command_builder_emits_golden_trigger_scan_envelope() {
    let builder = CommandBuilder::new(CMD_TRIGGER_SCAN, 1);
    assert_eq!(builder.finish(), &[0x01, 0x00, 0x01, 0x00][..]);
}

#[test]
fn command_builder_emits_golden_authenticate_associate_disconnect() {
    let mut builder = CommandBuilder::new(CMD_AUTHENTICATE, 2);
    builder
        .bssid(&[0x02, 0x00, 0x00, 0x00, 0x00, 0x01])
        .expect("bssid fits");
    assert_eq!(
        builder.finish(),
        &[
            0x04, 0x00, 0x02, 0x00, // cmd=AUTHENTICATE, seq=2
            0x02, 0x06, 0x02, 0x00, 0x00, 0x00, 0x00, 0x01, // BSSID attr
        ][..]
    );
    let mut builder = CommandBuilder::new(CMD_ASSOCIATE, 3);
    builder.ssid(b"ap").expect("ssid fits");
    assert_eq!(
        builder.finish(),
        &[0x05, 0x00, 0x03, 0x00, 0x01, 0x02, b'a', b'p'][..]
    );
    assert_eq!(
        CommandBuilder::new(CMD_DISCONNECT, 4).finish(),
        &[0x06, 0x00, 0x04, 0x00][..]
    );
}

#[test]
fn response_parser_walks_attributes_in_order() {
    let bytes: &[u8] = &[
        0x00, 0x00, // STATUS_OK
        0x07, 0x01, 0x02, // SECURITY = 2
        0x03, 0x01, 0x0B, // CHANNEL = 11
        0x01, 0x02, b'a', b'p', // SSID = "ap"
    ];
    let response = wireless::parse_response(bytes).expect("valid response");
    assert_eq!(response.status, STATUS_OK);
    let collected: Vec<(u8, usize)> = response
        .attrs()
        .map(|(id, payload)| (id, payload.len()))
        .collect();
    assert_eq!(collected, vec![(0x07, 1), (0x03, 1), (0x01, 2)]);
    assert_eq!(response.find(0x03), Some(&[0x0B][..]));
    assert_eq!(response.find(0x01), Some(&b"ap"[..]));
    assert_eq!(response.find(0x06), None);
}

#[test]
fn response_parser_rejects_malformed_envelopes() {
    let response = wireless::parse_response(&[0x01, 0x00]).expect("empty attrs valid");
    assert_eq!(response.status, STATUS_REJECTED);
    assert_eq!(response.attrs().count(), 0);

    assert!(matches!(
        wireless::parse_response(&[0x00]),
        Err(ParseError::TooShort)
    ));
    assert!(matches!(
        wireless::parse_response(&[0x00, 0x00, 0x01]),
        Err(ParseError::AttrTruncated)
    ));
    assert!(matches!(
        wireless::parse_response(&[0x00, 0x00, 0x01, 0x05, b'x']),
        Err(ParseError::BadAttrLength)
    ));
}

#[test]
fn command_builder_enforces_capacity_and_attr_limits() {
    // One attribute payload cannot exceed one length byte.
    let mut builder = CommandBuilder::new(CMD_TRIGGER_SCAN, 1);
    let big = [0u8; 300];
    assert_eq!(builder.attr(0x09, &big), Err(ParseError::BadAttrLength));

    // Saturating the 256-byte envelope eventually overflows.
    let mut builder = CommandBuilder::new(CMD_TRIGGER_SCAN, 1);
    let filler = [0x41u8; 100];
    let mut overflowed = false;
    for _ in 0..30 {
        if builder.attr(0x08, &filler).is_err() {
            overflowed = true;
            break;
        }
    }
    assert!(overflowed, "capacity must be enforced");

    // SSID helper caps at 32 octets; PSK helper at 64.
    let mut builder = CommandBuilder::new(CMD_TRIGGER_SCAN, 1);
    assert_eq!(builder.ssid(&[0u8; 33]), Err(ParseError::BadAttrLength));
    assert!(builder.ssid(&[0u8; 32]).is_ok());
    assert_eq!(builder.psk(&[0u8; 65]), Err(ParseError::BadAttrLength));
    assert!(builder.psk(&[0u8; 64]).is_ok());
}

// ---------------------------------------------------------------------------
// Scan-record decode (beacon / probe response)
// ---------------------------------------------------------------------------

/// Assembles a device-shaped scan record with a synthetic beacon body.
fn beacon_record(
    rssi: i8,
    record_channel: u8,
    ds_channel: Option<u8>,
    ssid: &[u8],
    rsne: Option<&[u8]>,
) -> Vec<u8> {
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(&0x0080u16.to_le_bytes()); // beacon mgmt frame
    body.extend_from_slice(&[0; 2]); // duration
    body.extend_from_slice(&[0xFF; 6]); // addr1 (DA, broadcast)
    body.extend_from_slice(&[0x02, 0x00, 0x00, 0x00, 0x00, 0x02]); // addr2 (SA)
    body.extend_from_slice(&[0x10, 0x20, 0x30, 0x40, 0x50, 0x60]); // addr3 (BSSID)
    body.extend_from_slice(&[0x00, 0x00]); // sequence control
    body.extend_from_slice(&[0; 8]); // timestamp
    body.extend_from_slice(&[0x64, 0x00]); // beacon interval
    body.extend_from_slice(&[0x11, 0x00]); // capability: ESS
    body.push(0x00); // SSID IE id
    body.push(ssid.len() as u8);
    body.extend_from_slice(ssid);
    if let Some(channel) = ds_channel {
        body.extend_from_slice(&[0x03, 0x01, channel]);
    }
    if let Some(rsne) = rsne {
        body.push(48);
        body.push(rsne.len() as u8);
        body.extend_from_slice(rsne);
    }
    let mut record: Vec<u8> = Vec::new();
    record.push(rssi as u8);
    record.push(record_channel);
    record.extend_from_slice(&(body.len() as u16).to_le_bytes());
    record.extend_from_slice(&body);
    record
}

/// RSNE payload with one pairwise suite and one AKM suite of `akm_type`.
fn rsne(akm_type: u8) -> Vec<u8> {
    let mut rsne: Vec<u8> = Vec::new();
    rsne.extend_from_slice(&1u16.to_le_bytes()); // version
    rsne.extend_from_slice(&[0x00, 0x0F, 0xAC, 0x04]); // group cipher CCMP
    rsne.extend_from_slice(&1u16.to_le_bytes()); // pairwise count
    rsne.extend_from_slice(&[0x00, 0x0F, 0xAC, 0x04]); // pairwise CCMP
    rsne.extend_from_slice(&1u16.to_le_bytes()); // akm count
    rsne.extend_from_slice(&[0x00, 0x0F, 0xAC, akm_type]);
    rsne.extend_from_slice(&[0x00, 0x00]); // RSN capabilities
    rsne
}

#[test]
fn scan_decode_classifies_open_network_with_bssid_and_channel() {
    let record = beacon_record(-55, 1, Some(6), b"open-net", None);
    let entry: ScanEntry = wireless::decode_scan_record(&record).expect("decodes");
    assert_eq!(entry.rssi, -55);
    assert_eq!(entry.bssid, [0x10, 0x20, 0x30, 0x40, 0x50, 0x60]);
    assert_eq!(entry.ssid, b"open-net");
    assert_eq!(
        entry.channel, 6,
        "DS parameter set must override record channel"
    );
    assert_eq!(entry.security, Security::Open);
}

#[test]
fn scan_decode_falls_back_to_record_channel_without_ds_ie() {
    let record = beacon_record(-70, 11, None, b"ch11", None);
    let entry = wireless::decode_scan_record(&record).expect("decodes");
    assert_eq!(entry.channel, 11);
}

#[test]
fn scan_decode_classifies_wpa2_and_wpa3_via_akm_suites() {
    let wpa2 = beacon_record(-40, 6, Some(6), b"wpa2-net", Some(&rsne(2)));
    let entry = wireless::decode_scan_record(&wpa2).expect("decodes");
    assert_eq!(entry.security, Security::Wpa2);

    let wpa3 = beacon_record(-40, 6, Some(6), b"wpa3-net", Some(&rsne(8)));
    let entry = wireless::decode_scan_record(&wpa3).expect("decodes");
    assert_eq!(entry.security, Security::Wpa3);
}

#[test]
fn scan_decode_flags_malformed_rsne_as_unknown() {
    // Bad version.
    let mut bad = rsne(2);
    bad[0] = 2;
    let record = beacon_record(-40, 6, None, b"broken", Some(&bad));
    let entry = wireless::decode_scan_record(&record).expect("decodes");
    assert_eq!(entry.security, Security::Unknown);

    // RSNE declaring far more bytes than the body carries.
    let record = beacon_record(-40, 6, None, b"br", None);
    let body_len = u16::from_le_bytes([record[2], record[3]]) as usize;
    let mut body = record[4..4 + body_len].to_vec();
    body.push(48);
    body.push(200);
    let mut rebuilt = vec![record[0], record[1]];
    rebuilt.extend_from_slice(&(body.len() as u16).to_le_bytes());
    rebuilt.extend_from_slice(&body);
    assert_eq!(
        wireless::decode_scan_record(&rebuilt),
        Err(DecodeError::IeTruncated)
    );
}

#[test]
fn scan_decode_rejects_structural_faults() {
    assert_eq!(
        wireless::decode_scan_record(&[]),
        Err(DecodeError::TooShort)
    );
    assert_eq!(
        wireless::decode_scan_record(&[0xF0, 11, 0x10, 0x00]),
        Err(DecodeError::BadBodyLength)
    );

    // Non-management frame control.
    let mut record = beacon_record(-50, 11, None, b"x", None);
    record[4] = 0x08; // data frame (type = 2)
    assert_eq!(
        wireless::decode_scan_record(&record),
        Err(DecodeError::NotManagementFrame)
    );

    // Fixed fields cut short: trim ten bytes off the body.
    let mut record = beacon_record(-50, 11, None, b"x", None);
    let body_len = u16::from_le_bytes([record[2], record[3]]);
    record.truncate(record.len() - 10);
    record[2..4].copy_from_slice(&(body_len - 10).to_le_bytes());
    assert_eq!(
        wireless::decode_scan_record(&record),
        Err(DecodeError::BadFixedFields)
    );
}

// ---------------------------------------------------------------------------
// Saved-network store codec
// ---------------------------------------------------------------------------

#[test]
fn saved_store_roundtrips_matrix_of_shapes() {
    let shapes: Vec<(Vec<u8>, Vec<u8>, Option<[u8; 6]>, u8)> = vec![
        (b"alpha".to_vec(), b"12345678".to_vec(), None, 1),
        (
            b"beta".to_vec(),
            [0x11u8; 32].to_vec(),
            Some([0xAA; 6]),
            200,
        ),
        (
            [0x00u8; 32].to_vec(),
            [0x22u8; 64].to_vec(),
            Some([0xBB; 6]),
            0,
        ),
    ];
    let mut store = SavedNetworkStore::new();
    for (ssid, psk, bssid, priority) in shapes.iter() {
        let record = SavedNetwork::new(ssid, psk, *bssid, *priority).expect("valid record");
        store.insert(record).expect("inserts");
    }
    assert_eq!(store.len(), 3);

    let mut encoded = [0u8; 1024];
    let used = store.encode(&mut encoded).expect("encodes");
    let decoded = SavedNetworkStore::decode(&encoded[..used]).expect("decodes");
    assert_eq!(decoded, store);

    let mut found_beta = false;
    for record in decoded.iter() {
        if record.ssid_bytes() == b"beta" {
            found_beta = true;
            assert_eq!(record.psk_bytes(), &[0x11u8; 32][..]);
            assert_eq!(record.bssid, Some([0xAA; 6]));
            assert_eq!(record.priority, 200);
        }
    }
    assert!(found_beta);
}

#[test]
fn saved_store_empty_and_single_record_codec_golden() {
    let empty = SavedNetworkStore::new();
    let mut out = [0u8; 64];
    assert_eq!(empty.encode(&mut out), Ok(4));
    assert_eq!(&out[..4], &[0x57, 0x53, 0x01, 0x00]);
    assert_eq!(SavedNetworkStore::decode(&out[..4]), Ok(empty));

    let mut store = SavedNetworkStore::new();
    store
        .insert(SavedNetwork::new(b"abcde", b"0123456789", None, 7).expect("valid"))
        .expect("inserts");
    let used = store.encode(&mut out).expect("encodes");
    // 4 header + (1+5) ssid + (1+10) psk + 1 bssid-flag + 1 priority
    assert_eq!(used, 23);
    assert_eq!(
        &out[..used],
        &[
            0x57, 0x53, 0x01, 0x01, // magic, version, count
            0x05, b'a', b'b', b'c', b'd', b'e', // ssid
            0x0A, b'0', b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9', // psk
            0x00, 0x07, // no bssid, priority
        ][..]
    );
}

#[test]
fn saved_store_replaces_same_ssid_and_rejects_overflow() {
    let mut store = SavedNetworkStore::new();
    let first = SavedNetwork::new(b"same", b"password1", None, 1).expect("valid");
    let second = SavedNetwork::new(b"other", b"password99", None, 2).expect("valid");
    store.insert(first).expect("inserts");
    store.insert(second).expect("inserts");
    let updated = SavedNetwork::new(b"same", b"99999999", Some([1; 6]), 9).expect("valid");
    store.insert(updated).expect("replaces in place");
    assert_eq!(store.len(), 2);
    let ssids: Vec<&[u8]> = store.iter().map(|record| record.ssid_bytes()).collect();
    assert_eq!(ssids, vec![&b"same"[..], &b"other"[..]]);
    let best = store.best().expect("non-empty");
    assert_eq!(best.priority, 9);
    assert_eq!(best.bssid, Some([1; 6]));

    // Capacity: fill to MAX and then refuse one more distinct SSID.
    let mut full = SavedNetworkStore::new();
    for index in 0..MAX_SAVED_NETWORKS {
        let ssid = [b'n', b'0' + index as u8, b'-', b'n', b'e', b't'];
        full.insert(SavedNetwork::new(&ssid, b"passphrase", None, 1).expect("valid"))
            .expect("inserts");
    }
    assert_eq!(full.len(), MAX_SAVED_NETWORKS);
    assert!(
        full.insert(SavedNetwork::new(b"overflow", b"passphrase", None, 1).expect("valid"))
            .is_none()
    );
}

#[test]
fn saved_store_removal_keeps_store_dense() {
    let mut store = SavedNetworkStore::new();
    for name in [b"one".as_slice(), b"two".as_slice(), b"three".as_slice()] {
        store
            .insert(SavedNetwork::new(name, b"passphrase", None, 1).expect("valid"))
            .expect("inserts");
    }
    assert!(store.remove(b"two"));
    assert_eq!(store.len(), 2);
    let ssids: Vec<&[u8]> = store.iter().map(|record| record.ssid_bytes()).collect();
    assert_eq!(ssids, vec![&b"one"[..], &b"three"[..]]);
    assert!(!store.remove(b"missing"));
    let mut encoded = [0u8; 256];
    let used = store.encode(&mut encoded).expect("encodes");
    assert_eq!(SavedNetworkStore::decode(&encoded[..used]), Ok(store));
}

#[test]
fn saved_store_best_resolves_priority_ties_by_insertion_order() {
    let mut store = SavedNetworkStore::new();
    for (ssid, priority) in [
        (b"low".as_slice(), 5u8),
        (b"high".as_slice(), 50u8),
        (b"equal".as_slice(), 50u8),
    ] {
        store
            .insert(SavedNetwork::new(ssid, b"passphrase", None, priority).expect("valid"))
            .expect("inserts");
    }
    assert_eq!(store.best().expect("non-empty").ssid_bytes(), b"high");
}

#[test]
fn saved_store_rejects_invalid_records_and_corrupt_codecs() {
    assert!(SavedNetwork::new(b"", b"passphrase", None, 1).is_none());
    assert!(SavedNetwork::new(b"ssid", b"", None, 1).is_none());
    assert!(SavedNetwork::new(&[0u8; 33], b"passphrase", None, 1).is_none());
    assert!(SavedNetwork::new(b"ssid", &[0u8; 65], None, 1).is_none());

    let mut store = SavedNetworkStore::new();
    store
        .insert(SavedNetwork::new(b"alpha", b"passphrase", None, 1).expect("valid"))
        .expect("inserts");
    let mut encoded = [0u8; 256];
    let used = store.encode(&mut encoded).expect("encodes");

    // Too-small destination.
    let mut tiny = [0u8; 8];
    assert_eq!(store.encode(&mut tiny), Err(CodecError::BufferTooSmall));

    // Corruptions.
    let mut corrupt = encoded;
    corrupt[0] = 0x00;
    assert_eq!(
        SavedNetworkStore::decode(&corrupt[..used]),
        Err(CodecError::BadMagic)
    );
    let mut corrupt = encoded;
    corrupt[2] = 9;
    assert_eq!(
        SavedNetworkStore::decode(&corrupt[..used]),
        Err(CodecError::BadVersion)
    );
    let mut corrupt = encoded;
    corrupt[3] = MAX_SAVED_NETWORKS as u8 + 1;
    assert_eq!(
        SavedNetworkStore::decode(&corrupt[..used]),
        Err(CodecError::BadRecord)
    );
    assert_eq!(
        SavedNetworkStore::decode(&encoded[..used - 1]),
        Err(CodecError::BadRecord)
    );

    // Trailing bytes after the declared count.
    let mut trailing = [0u8; 256];
    trailing[..used].copy_from_slice(&encoded[..used]);
    trailing[used] = 0xFF;
    assert_eq!(
        SavedNetworkStore::decode(&trailing[..used + 1]),
        Err(CodecError::BadRecord)
    );
}

// ---------------------------------------------------------------------------
// Integrity-grade key material (RFC 4231 HMAC-SHA-512 golden vectors)
// ---------------------------------------------------------------------------

fn hex_to_64(hex: &str) -> [u8; 64] {
    let mut out = [0u8; 64];
    for index in 0..64 {
        out[index] = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).expect("hex byte");
    }
    out
}

#[test]
fn hmac_sha512_matches_rfc_4231_vectors() {
    // Test case 1: key 0x0b x20, data "Hi There".
    assert_eq!(
        wireless::hmac_sha512(&[0x0bu8; 20], &[b"Hi There"]),
        hex_to_64(
            "87aa7cdea5ef619d4ff0b4241a1d6cb02379f4e2ce4ec2787ad0b30545e17cdedaa833b7d6b8a702038b274eaea3f4e4be9d914eeb61f1702e696c203a126854"
        )
    );

    // Test case 2: key "Jefe", data "what do ya want for nothing?".
    assert_eq!(
        wireless::hmac_sha512(b"Jefe", &[b"what do ya want for nothing?"]),
        hex_to_64(
            "164b7a7bfcf819e2e395fbe73b56e0a387bd64222e831fd610270cd7ea2505549758bf75c05a994a6d034f65f8f0e6fdcaeab1a34d4a6b4b636e070a38bce737"
        )
    );

    // Test case 3: 131-byte key (exercises the hash-then-pad long-key path),
    // 50 bytes of 0xdd.
    assert_eq!(
        wireless::hmac_sha512(&[0xaau8; 131], &[&[0xddu8; 50]]),
        hex_to_64(
            "0561688775ef4645d176e8334d4f568724dbf3a9409bb6495097bd61afa684f0b7c6471dabd5b68d3a0b080a465ed9fadb8b2541ba295b38100323b6c00bb85b"
        )
    );
}

#[test]
fn ptk_and_pmk_placeholders_are_deterministic_and_binding() {
    let pmk = wireless::pmk_from_psk_placeholder(b"passphrase-here", b"my-ssid");
    assert_eq!(
        pmk,
        wireless::pmk_from_psk_placeholder(b"passphrase-here", b"my-ssid")
    );
    assert_ne!(
        pmk,
        wireless::pmk_from_psk_placeholder(b"passphrase-here", b"other-ssid")
    );
    assert_ne!(
        pmk,
        wireless::pmk_from_psk_placeholder(b"other-passphrase", b"my-ssid")
    );

    let aa = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];
    let spa = [0x02, 0x00, 0x00, 0x00, 0x00, 0x02];
    let anonce = [0xA5u8; 32];
    let snonce = [0x5Au8; 32];

    let ptk = wireless::derive_ptk_placeholder(&pmk, &aa, &spa, &anonce, &snonce);
    assert_eq!(
        wireless::derive_ptk_placeholder(&pmk, &aa, &spa, &anonce, &snonce),
        ptk
    );
    // Address binding: swapping AA and SPA changes the key.
    assert_ne!(
        wireless::derive_ptk_placeholder(&pmk, &spa, &aa, &anonce, &snonce),
        ptk
    );
    // Nonce binding.
    assert_ne!(
        wireless::derive_ptk_placeholder(&pmk, &aa, &spa, &snonce, &snonce),
        ptk
    );
    // Key thirds are distinct material.
    assert_ne!(ptk.kck, ptk.kek);
    assert_ne!(ptk.kek, ptk.tk);
    assert_ne!(ptk.kck, ptk.tk);
}

#[test]
fn eapol_mic_placeholder_matches_truncated_hmac_shape() {
    let kck = [0x42u8; 16];
    let covered: &[u8] = b"covered bytes";
    let mic = wireless::eapol_mic_placeholder(&kck, &[covered]);
    let digest = wireless::hmac_sha512(&kck, &[covered]);
    assert_eq!(&mic[..], &digest[..16]);
    assert_ne!(
        mic,
        wireless::eapol_mic_placeholder(&kck, &[b"other bytes"])
    );
}

// ---------------------------------------------------------------------------
// WPA2-PSK 4-way handshake (authenticator side)
// ---------------------------------------------------------------------------

const AA: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];
const SPA: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x02];
const ANONCE: [u8; 32] = [0xA5u8; 32];
const SNONCE: [u8; 32] = [0x5Au8; 32];
const KEY_DATA: &[u8] = b"gtk-material";

/// Supplicant stand-in: computes the placeholder MIC exactly like a peer.
fn peer_mic(authenticator: &Authenticator, kind: u8, replay: u64, payload: &[u8]) -> [u8; 16] {
    let kck = &authenticator.ptk().expect("ptk").kck;
    let kind_bytes = [kind];
    let replay_bytes = replay.to_be_bytes();
    wireless::eapol_mic_placeholder(kck, &[&kind_bytes, &replay_bytes, payload])
}

#[test]
fn handshake_happy_path_reaches_installed_with_ptk() {
    let pmk = wireless::pmk_from_psk_placeholder(b"passphrase-here", b"my-ssid");
    let mut authenticator = Authenticator::new(pmk, AA, SPA);
    assert_eq!(authenticator.state(), HandshakeState::AwaitingMessage1);
    assert!(authenticator.ptk().is_none());

    authenticator.send_message1(ANONCE);
    assert_eq!(authenticator.state(), HandshakeState::AwaitingMessage2);
    assert!(authenticator.ptk().is_some(), "PTK derived at message 1");
    // SNonce derivation is deterministic: a fresh authenticator agrees.
    let fresh = Authenticator::new(pmk, AA, SPA);
    let mut fresh = fresh;
    fresh.send_message1(ANONCE);
    assert_eq!(authenticator.snonce(), fresh.snonce());
    let snonce = *authenticator.snonce();

    // Message 2: correct replay, correct MIC slot.
    let message2 = EapolKeyFrame {
        kind: wireless::EAPOL_MESSAGE_2,
        replay: 1,
        nonce: Some(snonce),
        mic: Some(peer_mic(
            &authenticator,
            wireless::EAPOL_MESSAGE_2,
            1,
            KEY_DATA,
        )),
        payload_len: KEY_DATA.len(),
    };
    authenticator
        .on_message2(&message2, KEY_DATA)
        .expect("message 2 verifies");
    assert_eq!(authenticator.state(), HandshakeState::Message2Verified);

    // Message 3: replay 2, MIC over its own key data.
    let message3 = authenticator
        .send_message3(KEY_DATA)
        .expect("emits message 3");
    assert_eq!(message3.replay, 2);
    assert_eq!(authenticator.state(), HandshakeState::AwaitingMessage4);

    // Message 4: carries no key data; MIC over empty payload.
    let message4 = EapolKeyFrame {
        kind: wireless::EAPOL_MESSAGE_4,
        replay: 2,
        nonce: None,
        mic: Some(peer_mic(&authenticator, wireless::EAPOL_MESSAGE_4, 2, &[])),
        payload_len: 0,
    };
    authenticator
        .on_message4(&message4, &[])
        .expect("message 4 verifies");
    assert_eq!(authenticator.state(), HandshakeState::Installed);
}

#[test]
fn handshake_wire_roundtrip_preserves_frames() {
    let mut buffer = [0u8; 128];
    let frame = EapolKeyFrame {
        kind: wireless::EAPOL_MESSAGE_1,
        replay: 0x0102_0304_0506_0708,
        nonce: Some(ANONCE),
        mic: None,
        payload_len: 3,
    };
    let used = frame.encode(b"abc", &mut buffer).expect("encodes");
    assert_eq!(used, 9 + 32 + 2 + 3);
    let (decoded, payload) = EapolKeyFrame::decode(&buffer[..used]).expect("decodes");
    assert_eq!(decoded.kind, frame.kind);
    assert_eq!(decoded.replay, 0x0102_0304_0506_0708);
    assert_eq!(decoded.nonce, Some(ANONCE));
    assert_eq!(decoded.mic, None);
    assert_eq!(payload, b"abc");

    // MIC-carrying frame without nonce (message 3 shape).
    let frame = EapolKeyFrame {
        kind: wireless::EAPOL_MESSAGE_3,
        replay: 2,
        nonce: None,
        mic: Some([7u8; 16]),
        payload_len: 1,
    };
    let used = frame.encode(b"x", &mut buffer).expect("encodes");
    let (decoded, payload) = EapolKeyFrame::decode(&buffer[..used]).expect("decodes");
    assert_eq!(decoded, frame);
    assert_eq!(payload, b"x");
}

#[test]
fn handshake_wire_decode_rejects_faults() {
    let mut buffer = [0u8; 128];
    let frame = EapolKeyFrame {
        kind: wireless::EAPOL_MESSAGE_2,
        replay: 1,
        nonce: Some(SNONCE),
        mic: Some([0; 16]),
        payload_len: 2,
    };
    let used = frame.encode(b"ok", &mut buffer).expect("encodes");

    // Payload length disagreement (corrupt the high length byte).
    let mut corrupt = buffer;
    corrupt[used - 4] ^= 0xFF;
    assert_eq!(
        EapolKeyFrame::decode(&corrupt[..used]),
        Err(WireError::BadPayloadLength)
    );
    // Truncations.
    assert_eq!(
        EapolKeyFrame::decode(&buffer[..8]),
        Err(WireError::TooShort)
    );
    assert_eq!(
        EapolKeyFrame::decode(&buffer[..40]),
        Err(WireError::TooShort)
    );
    // Bad kind.
    let mut corrupt = buffer;
    corrupt[0] = 9;
    assert_eq!(
        EapolKeyFrame::decode(&corrupt[..used]),
        Err(WireError::BadKind)
    );
    // Buffer too small.
    let mut tiny = [0u8; 8];
    assert_eq!(
        frame.encode(b"ok", &mut tiny),
        Err(WireError::BufferTooSmall)
    );
    // MIC-carrying kind without a MIC slot.
    let broken = EapolKeyFrame {
        kind: wireless::EAPOL_MESSAGE_2,
        replay: 1,
        nonce: Some(SNONCE),
        mic: None,
        payload_len: 0,
    };
    assert_eq!(broken.encode(b"", &mut buffer), Err(WireError::BadKind));
}

#[test]
fn handshake_rejects_replay_and_mic_faults_at_every_step() {
    let pmk = wireless::pmk_from_psk_placeholder(b"passphrase-here", b"my-ssid");
    let mut authenticator = Authenticator::new(pmk, AA, SPA);
    authenticator.send_message1(ANONCE);

    // Wrong replay counter on message 2.
    let replay = EapolKeyFrame {
        kind: wireless::EAPOL_MESSAGE_2,
        replay: 99,
        nonce: Some(*authenticator.snonce()),
        mic: Some(peer_mic(
            &authenticator,
            wireless::EAPOL_MESSAGE_2,
            1,
            KEY_DATA,
        )),
        payload_len: KEY_DATA.len(),
    };
    assert_eq!(
        authenticator.on_message2(&replay, KEY_DATA),
        Err(HandshakeError::ReplayMismatch)
    );
    assert_eq!(authenticator.state(), HandshakeState::AwaitingMessage2);

    // Corrupted MIC slot on message 2.
    let mut bad_mic = peer_mic(&authenticator, wireless::EAPOL_MESSAGE_2, 1, KEY_DATA);
    bad_mic[0] ^= 0x80;
    let corrupt = EapolKeyFrame {
        kind: wireless::EAPOL_MESSAGE_2,
        replay: 1,
        nonce: Some(*authenticator.snonce()),
        mic: Some(bad_mic),
        payload_len: KEY_DATA.len(),
    };
    assert_eq!(
        authenticator.on_message2(&corrupt, KEY_DATA),
        Err(HandshakeError::MicMismatch)
    );

    // Wrong message type in the awaiting-message-2 phase.
    let wrong_kind = EapolKeyFrame {
        kind: wireless::EAPOL_MESSAGE_4,
        replay: 1,
        nonce: None,
        mic: Some([0; 16]),
        payload_len: 0,
    };
    assert_eq!(
        authenticator.on_message2(&wrong_kind, &[]),
        Err(HandshakeError::WrongMessageType)
    );

    // Correct message 2, then message 4 before message 3 is refused.
    let good = EapolKeyFrame {
        kind: wireless::EAPOL_MESSAGE_2,
        replay: 1,
        nonce: Some(*authenticator.snonce()),
        mic: Some(peer_mic(
            &authenticator,
            wireless::EAPOL_MESSAGE_2,
            1,
            KEY_DATA,
        )),
        payload_len: KEY_DATA.len(),
    };
    authenticator
        .on_message2(&good, KEY_DATA)
        .expect("message 2 verified");
    assert_eq!(authenticator.state(), HandshakeState::Message2Verified);
    assert_eq!(
        authenticator.on_message4(&wrong_kind, &[]),
        Err(HandshakeError::WrongState)
    );

    let message3 = authenticator
        .send_message3(KEY_DATA)
        .expect("emits message 3");
    assert_eq!(message3.replay, 2);

    // Replayed message 2 now lands in the wrong phase.
    assert_eq!(
        authenticator.on_message2(&good, KEY_DATA),
        Err(HandshakeError::WrongState)
    );

    // Message 4 with the message-3 replay counter is a replay.
    let stale = EapolKeyFrame {
        kind: wireless::EAPOL_MESSAGE_4,
        replay: 1,
        nonce: None,
        mic: Some(peer_mic(&authenticator, wireless::EAPOL_MESSAGE_4, 1, &[])),
        payload_len: 0,
    };
    assert_eq!(
        authenticator.on_message4(&stale, &[]),
        Err(HandshakeError::ReplayMismatch)
    );

    // Corrupted message-4 MIC.
    let mut bad_four = peer_mic(&authenticator, wireless::EAPOL_MESSAGE_4, 2, &[]);
    bad_four[15] ^= 0x80;
    let corrupt = EapolKeyFrame {
        kind: wireless::EAPOL_MESSAGE_4,
        replay: 2,
        nonce: None,
        mic: Some(bad_four),
        payload_len: 0,
    };
    assert_eq!(
        authenticator.on_message4(&corrupt, &[]),
        Err(HandshakeError::MicMismatch)
    );
    assert_eq!(authenticator.state(), HandshakeState::AwaitingMessage4);

    // Good message 4 installs; message 3 emission is now out of phase.
    let good4 = EapolKeyFrame {
        kind: wireless::EAPOL_MESSAGE_4,
        replay: 2,
        nonce: None,
        mic: Some(peer_mic(&authenticator, wireless::EAPOL_MESSAGE_4, 2, &[])),
        payload_len: 0,
    };
    authenticator.on_message4(&good4, &[]).expect("installs");
    assert_eq!(authenticator.state(), HandshakeState::Installed);
    assert_eq!(
        authenticator.send_message3(KEY_DATA),
        Err(HandshakeError::WrongState)
    );
}

#[test]
fn handshake_rejects_wrong_state_at_entry() {
    let pmk = wireless::pmk_from_psk_placeholder(b"passphrase-here", b"my-ssid");
    let mut authenticator = Authenticator::new(pmk, AA, SPA);
    let frame = EapolKeyFrame {
        kind: wireless::EAPOL_MESSAGE_2,
        replay: 1,
        nonce: Some(SNONCE),
        mic: Some([0; 16]),
        payload_len: 0,
    };
    // Message 2 before message 1.
    assert_eq!(
        authenticator.on_message2(&frame, &[]),
        Err(HandshakeError::WrongState)
    );
    assert_eq!(
        authenticator.send_message3(&[]),
        Err(HandshakeError::WrongState)
    );
    authenticator.send_message1(ANONCE);
    // Message 3 emission is illegal before message 2 verifies.
    assert_eq!(
        authenticator.send_message3(&[]),
        Err(HandshakeError::WrongState)
    );
}

// ---------------------------------------------------------------------------
// Link-state machine
// ---------------------------------------------------------------------------

#[test]
fn link_machine_walks_the_full_happy_path() {
    let mut link = LinkMonitor::new();
    assert_eq!(link.state(), LinkState::Down);
    assert_eq!(
        link.advance(LinkEvent::ScanStarted),
        Ok(LinkState::Scanning)
    );
    // Scan window closes without forcing a transition.
    assert_eq!(
        link.advance(LinkEvent::ScanComplete),
        Ok(LinkState::Scanning)
    );
    assert_eq!(
        link.advance(LinkEvent::JoinRequested),
        Ok(LinkState::Authenticating)
    );
    assert_eq!(link.advance(LinkEvent::AuthOk), Ok(LinkState::Associating));
    assert_eq!(link.advance(LinkEvent::AssocOk), Ok(LinkState::Connected));
    assert_eq!(link.state(), LinkState::Connected);
}

#[test]
fn link_machine_times_out_transient_states_to_down() {
    const WALK: [LinkEvent; 3] = [
        LinkEvent::ScanStarted,
        LinkEvent::JoinRequested,
        LinkEvent::AuthOk,
    ];
    // Depths 1..=3 land in Scanning / Authenticating / Associating; each
    // transient phase times out to Down.
    for depth in 1..=WALK.len() {
        let mut link = LinkMonitor::new();
        for step in WALK.iter().take(depth) {
            link.advance(*step).expect("legal");
        }
        assert_eq!(
            link.on_timeout(),
            LinkState::Down,
            "transient phase times out"
        );
        // Down is stable against further ticks.
        assert_eq!(link.on_timeout(), LinkState::Down);
    }
    let mut link = LinkMonitor::new();
    assert_eq!(link.on_timeout(), LinkState::Down);
    for step in WALK.iter() {
        link.advance(*step).expect("legal");
    }
    link.advance(LinkEvent::AssocOk).expect("legal");
    assert_eq!(
        link.on_timeout(),
        LinkState::Connected,
        "connected has no idle timeout"
    );
}

#[test]
fn link_machine_deauths_from_every_state() {
    const WALK: [LinkEvent; 4] = [
        LinkEvent::ScanStarted,
        LinkEvent::JoinRequested,
        LinkEvent::AuthOk,
        LinkEvent::AssocOk,
    ];
    // Depth 0 = Down; depths 1..=4 cover every phase of the chain.
    for depth in 0..=WALK.len() {
        let mut link = LinkMonitor::new();
        for step in WALK.iter().take(depth) {
            link.advance(*step).expect("legal");
        }
        assert_eq!(link.advance(LinkEvent::Deauth), Ok(LinkState::Down));
        assert_eq!(link.state(), LinkState::Down);
    }
}

#[test]
fn link_machine_rejects_illegal_transitions_without_state_change() {
    let mut link = LinkMonitor::new();
    // Connect without walking the chain.
    assert_eq!(link.advance(LinkEvent::AssocOk), Err(LinkStateError));
    assert_eq!(link.state(), LinkState::Down);
    // Re-scan while connected is not modeled.
    link.advance(LinkEvent::ScanStarted).expect("legal");
    link.advance(LinkEvent::JoinRequested).expect("legal");
    link.advance(LinkEvent::AuthOk).expect("legal");
    link.advance(LinkEvent::AssocOk).expect("legal");
    assert_eq!(link.advance(LinkEvent::ScanStarted), Err(LinkStateError));
    assert_eq!(link.state(), LinkState::Connected);
    // Double join is refused while connected.
    assert_eq!(link.advance(LinkEvent::JoinRequested), Err(LinkStateError));
    assert_eq!(link.state(), LinkState::Connected);
    // Double join is also refused while authenticating.
    let mut link = LinkMonitor::new();
    link.advance(LinkEvent::ScanStarted).expect("legal");
    link.advance(LinkEvent::JoinRequested).expect("legal");
    assert_eq!(link.advance(LinkEvent::JoinRequested), Err(LinkStateError));
    assert_eq!(link.state(), LinkState::Authenticating);
    // Deauth out of Down stays Down and is legal (idempotent reset).
    let mut link = LinkMonitor::new();
    assert_eq!(link.advance(LinkEvent::Deauth), Ok(LinkState::Down));
}
