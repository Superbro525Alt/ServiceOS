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
//! - scan records, the saved-network store codec and the link state machine
//!   (extracted verbatim into the shared `serviceos-wireless` crate and
//!   re-exported below so the service layer can share them);
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

// SSID/PSK/saved-network limits and the scan-record, saved-network-store and
// link-FSM pieces are extracted verbatim into the shared `serviceos-wireless`
// crate (service layer consumes them without a platform dependency) and are
// re-exported here so this module's public surface and host test harness stay
// byte-for-byte compatible.
pub use serviceos_wireless::{
    CodecError, DecodeError, LinkEvent, LinkMonitor, LinkState, LinkStateError, MAX_PSK_LEN,
    MAX_SAVED_NETWORKS, MAX_SSID_LEN, SavedNetwork, SavedNetworkStore, ScanEntry, Security,
    decode_scan_record,
};

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
// Integrity-grade key material (HMAC / PRF placeholders)
// ---------------------------------------------------------------------------

/// HMAC-SHA-512 (RFC 2104 block layout, block size 128).
///
/// Re-exported from `serviceos_crypto::hmac` — the pure implementation was
/// moved there so exactly one HMAC exists in the tree; this re-export keeps
/// the historical `wireless::hmac_sha512` path working for callers and
/// tests. INTEGRITY-GRADE HONESTY NOTE: real WPA2 key derivation uses
/// HMAC-SHA-1 (PRF-512, 802.11i) and AES-CMAC MICs (802.11w/CCMP); this
/// SHA-512 HMAC is a *placeholder* of the same shape so the protocol layer
/// is complete and testable. It is cryptographically sound as an HMAC but
/// is NOT interoperable with real 802.11i peers, and MUST be replaced with
/// the spec algorithms before hardware bring-up.
pub use serviceos_crypto::hmac::hmac_sha512;

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
