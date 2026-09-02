//! Binary packet protocol (RFC 4253 §6) in two framings:
//!
//! * **plain** (pre-KEX): `u32 packet_length | u8 padding_length | payload |
//!   padding`, zero padding bytes, block size 8, no MAC.
//! * **AEAD** (`chacha20-poly1305@openssh.com`): the 4-byte packet_length is
//!   encrypted with the direction's *header* ChaCha20 key (counter 0); the
//!   payload+padding is encrypted and authenticated with the *main* key via
//!   ChaCha20-Poly1305 with the **encrypted length bytes as AAD**; a 16-byte
//!   tag follows; there is no separate MAC field. The AEAD nonce for both
//!   ChaCha instances is `0x00000000 || u64be(sequence number)` — the nonce
//!   is thus 12 bytes with the 8-byte big-endian sequence number zero-padded
//!   on the left (OpenSSH PROTOCOL.chacha20poly1305).
//!
//! Padding rule (both framings): `packet_length = 1 + payload + padding`,
//! `(4 + packet_length) % block_size == 0`, `padding >= 4`. Deviation
//! documented: padding is zero-filled because the library is pure (no RNG);
//! padding entropy is a traffic-analysis nicety, not a correctness
//! requirement. Callers may pre-fill randomness if they extend the API.

use crate::error::DisconnectReason;
use crate::wire::WireErr;
use serviceos_crypto::chacha20;

/// Block size for both framings (RFC 4253 §6 arbitrary-size min 8; the
/// openssh AEAD cipher also specifies block size 8).
pub const BLOCK_SIZE: usize = 8;
/// Minimum padding (RFC 4253 §6).
pub const MIN_PADDING: usize = 4;
/// Maximum accepted `packet_length` (RFC 4253 §6.1 recommends 35000 support).
pub const MAX_PACKET_LEN: usize = 35000;
/// Maximum payload we accept or emit.
pub const MAX_PAYLOAD_LEN: usize = 32768;
/// Largest wire packet: 4 (length) + MAX_PACKET_LEN + 16 (tag).
pub const MAX_WIRE_LEN: usize = 4 + MAX_PACKET_LEN + 16;

/// Direction cipher keys for `chacha20-poly1305@openssh.com`: 32-byte main
/// (payload) key + 32-byte header (length) key — 64 bytes of KDF output.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CipherKeys {
    pub main: [u8; 32],
    pub header: [u8; 32],
}

impl CipherKeys {
    /// Split a 64-byte KDF output: first 32 = main, second 32 = header.
    pub fn from_material(material: &[u8; 64]) -> CipherKeys {
        let mut main = [0u8; 32];
        let mut header = [0u8; 32];
        main.copy_from_slice(&material[..32]);
        header.copy_from_slice(&material[32..]);
        CipherKeys { main, header }
    }
}

/// AEAD nonce construction: 4 zero octets then the sequence number as an
/// 8-byte big-endian integer (OpenSSH chacha20-poly1305 spec).
pub fn nonce_for_seqno(seqno: u32) -> [u8; 12] {
    let be = (seqno as u64).to_be_bytes();
    [
        0, 0, 0, 0, be[0], be[1], be[2], be[3], be[4], be[5], be[6], be[7],
    ]
}

/// Compute the RFC 4253 §6 padding length for a payload under `block_size`.
pub fn padding_len(payload_len: usize, block_size: usize) -> usize {
    // packet_length = 1 + payload + padding; (4 + packet_length) % B == 0.
    let base = (4 + 1 + payload_len) % block_size;
    let pad = block_size - base;
    if pad < MIN_PADDING {
        pad + block_size
    } else {
        pad
    }
}

/// Encode a plain (pre-KEX) packet: `length | padlen | payload | padding`
/// into `out`; returns the wire length. Padding is zero-filled (see module
/// docs). `payload.len()` must be ≤ `MAX_PAYLOAD_LEN`.
pub fn encode_plain(payload: &[u8], out: &mut [u8]) -> Result<usize, WireErr> {
    if payload.len() > MAX_PAYLOAD_LEN {
        return Err(WireErr::Overflow);
    }
    let pad = padding_len(payload.len(), BLOCK_SIZE);
    let packet_length = 1 + payload.len() + pad;
    let total = 4 + packet_length;
    if out.len() < total {
        return Err(WireErr::Overflow);
    }
    out[0..4].copy_from_slice(&(packet_length as u32).to_be_bytes());
    out[4] = pad as u8;
    out[5..5 + payload.len()].copy_from_slice(payload);
    out[5 + payload.len()..total].fill(0);
    Ok(total)
}

/// Encode an AEAD packet whose padded message (`padlen | payload | padding`)
/// is already assembled in `msg` (packet_length == msg.len()). Used by the
/// transport, which stages outgoing payloads to avoid overlapping borrows.
/// Returns the wire length (`4 + packet_length + 16`).
pub fn encode_aead_msg(
    msg: &[u8],
    keys: &CipherKeys,
    seqno: u32,
    out: &mut [u8],
) -> Result<usize, WireErr> {
    if msg.is_empty() || msg.len() > MAX_PACKET_LEN {
        return Err(WireErr::Overflow);
    }
    let packet_length = msg.len();
    let wire_len = 4 + packet_length + 16;
    if out.len() < wire_len {
        return Err(WireErr::Overflow);
    }
    let nonce = nonce_for_seqno(seqno);
    let len_be = (packet_length as u32).to_be_bytes();
    let (head, body) = out.split_at_mut(4);
    chacha20::xor(&keys.header, 0, &nonce, &len_be, head);
    let tag = serviceos_crypto::chacha20poly1305::encrypt(
        &keys.main,
        &nonce,
        head,
        msg,
        &mut body[..packet_length],
    );
    body[packet_length..packet_length + 16].copy_from_slice(&tag);
    Ok(wire_len)
}

/// Encode an AEAD (`chacha20-poly1305@openssh.com`) packet into `out`.
/// `scratch` receives the unencrypted `padlen | payload | padding` message
/// (must be at least `1 + payload.len() + padding_len(...)` bytes).
/// Returns the wire length (`4 + packet_length + 16`).
pub fn encode_aead(
    payload: &[u8],
    keys: &CipherKeys,
    seqno: u32,
    scratch: &mut [u8],
    out: &mut [u8],
) -> Result<usize, WireErr> {
    if payload.len() > MAX_PAYLOAD_LEN {
        return Err(WireErr::Overflow);
    }
    let pad = padding_len(payload.len(), BLOCK_SIZE);
    let msg_len = 1 + payload.len() + pad;
    if scratch.len() < msg_len {
        return Err(WireErr::Overflow);
    }
    let packet_length = msg_len;
    let wire_len = 4 + packet_length + 16;
    if out.len() < wire_len {
        return Err(WireErr::Overflow);
    }
    let nonce = nonce_for_seqno(seqno);

    // Message = padlen | payload | padding.
    scratch[0] = pad as u8;
    scratch[1..1 + payload.len()].copy_from_slice(payload);
    scratch[1 + payload.len()..msg_len].fill(0);

    // Encrypted length field via the header key, ChaCha counter 0.
    let len_be = (packet_length as u32).to_be_bytes();
    let (head, body) = out.split_at_mut(4);
    chacha20::xor(&keys.header, 0, &nonce, &len_be, head);
    // Payload AEAD with the encrypted length as AAD.
    let tag = serviceos_crypto::chacha20poly1305::encrypt(
        &keys.main,
        &nonce,
        head,
        &scratch[..msg_len],
        &mut body[..msg_len],
    );
    body[msg_len..msg_len + 16].copy_from_slice(&tag);
    Ok(wire_len)
}

/// A decoded frame: indices into the decode staging buffer plus how many
/// wire bytes were consumed from the input stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameInfo {
    /// Wire bytes consumed from the front of the accumulator.
    pub consumed: usize,
    /// Bytes in the staging buffer (padlen byte + payload + padding).
    pub staged: usize,
    /// Payload start inside the staging buffer (always ≥ 1).
    pub payload_start: usize,
    /// Payload length inside the staging buffer.
    pub payload_len: usize,
}

impl FrameInfo {
    pub fn msg_type<'a>(&self, staging: &'a [u8]) -> u8 {
        staging[self.payload_start]
    }
    pub fn payload<'a>(&self, staging: &'a [u8]) -> &'a [u8] {
        &staging[self.payload_start..self.payload_start + self.payload_len]
    }
}

/// Validate the decrypted `padlen | payload | padding` message.
/// Returns the payload range inside `staged`.
fn validate_staged(
    staged: &[u8],
    packet_length: usize,
) -> Result<(usize, usize), DisconnectReason> {
    if packet_length == 0 || staged.len() < packet_length {
        return Err(DisconnectReason::ProtocolError);
    }
    if (4 + packet_length) % BLOCK_SIZE != 0 {
        return Err(DisconnectReason::ProtocolError);
    }
    let pad = staged[0] as usize;
    if pad < MIN_PADDING || 1 + pad > packet_length {
        return Err(DisconnectReason::ProtocolError);
    }
    let payload_len = packet_length - 1 - pad;
    if payload_len > MAX_PAYLOAD_LEN {
        return Err(DisconnectReason::ProtocolError);
    }
    Ok((1, payload_len))
}

/// Decode a plain packet from the front of `buf`. `Ok(None)` = need more
/// bytes. The staged bytes (`padlen | payload | padding`, padding included)
/// are copied into `staging`.
pub fn decode_plain(buf: &[u8], staging: &mut [u8]) -> Result<Option<FrameInfo>, DisconnectReason> {
    if buf.len() < 4 {
        return Ok(None);
    }
    let packet_length = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    if packet_length == 0 || packet_length > MAX_PACKET_LEN {
        return Err(DisconnectReason::ProtocolError);
    }
    let needed = 4 + packet_length;
    if buf.len() < needed {
        return Ok(None);
    }
    staging[..packet_length].copy_from_slice(&buf[4..needed]);
    let (start, payload_len) = validate_staged(&staging[..packet_length], packet_length)?;
    Ok(Some(FrameInfo {
        consumed: needed,
        staged: packet_length,
        payload_start: start,
        payload_len,
    }))
}

/// Decode an AEAD packet: decrypt the length field with the header key,
/// bounds-check, then AEAD-decrypt/verify the message (AAD = the 4 encrypted
/// length bytes). On tag mismatch returns `Err(MacError)` without writing
/// plaintext beyond the staging prefix (decrypt is release-on-verified).
pub fn decode_aead(
    buf: &[u8],
    staging: &mut [u8],
    keys: &CipherKeys,
    seqno: u32,
    len_scratch: &mut [u8; 4],
) -> Result<Option<FrameInfo>, DisconnectReason> {
    if buf.len() < 4 {
        return Ok(None);
    }
    let nonce = nonce_for_seqno(seqno);
    chacha20::xor(&keys.header, 0, &nonce, &buf[0..4], len_scratch);
    let packet_length = u32::from_be_bytes(*len_scratch) as usize;
    if packet_length == 0 || packet_length > MAX_PACKET_LEN || (4 + packet_length) % BLOCK_SIZE != 0
    {
        return Err(DisconnectReason::ProtocolError);
    }
    let needed = 4 + packet_length + 16;
    if buf.len() < needed {
        return Ok(None);
    }
    let ok = serviceos_crypto::chacha20poly1305::decrypt(
        &keys.main,
        &nonce,
        &buf[0..4],
        &buf[4..4 + packet_length],
        &buf[4 + packet_length..needed].try_into().unwrap(),
        &mut staging[..packet_length],
    );
    if !ok {
        return Err(DisconnectReason::MacError);
    }
    let (start, payload_len) = validate_staged(&staging[..packet_length], packet_length)?;
    Ok(Some(FrameInfo {
        consumed: needed,
        staged: packet_length,
        payload_start: start,
        payload_len,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(seed: u8) -> CipherKeys {
        let mut mat = [0u8; 64];
        mat.iter_mut()
            .enumerate()
            .for_each(|(i, b)| *b = seed.wrapping_add(i as u8));
        CipherKeys::from_material(&mat)
    }

    #[test]
    fn nonce_construction() {
        assert_eq!(nonce_for_seqno(0), [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(nonce_for_seqno(1), [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        assert_eq!(
            nonce_for_seqno(0x0102_0304),
            [0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4]
        );
        // Wrap edge: u32::MAX zero-padded into the low 8 bytes.
        assert_eq!(
            nonce_for_seqno(u32::MAX),
            [0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]
        );
    }

    #[test]
    fn seqno_wrapping_increment() {
        let mut seq = u32::MAX;
        seq = seq.wrapping_add(1);
        assert_eq!(seq, 0);
    }

    #[test]
    fn padding_invariants() {
        for len in [0usize, 1, 4, 7, 8, 9, 100, 1000] {
            let pad = padding_len(len, BLOCK_SIZE);
            assert!(pad >= 4);
            assert_eq!((4 + 1 + len + pad) % BLOCK_SIZE, 0);
        }
    }

    #[test]
    fn plain_roundtrip() {
        let mut out = [0u8; 256];
        let n = encode_plain(b"hello world", &mut out).unwrap();
        let mut staging = [0u8; 256];
        let f = decode_plain(&out[..n], &mut staging).unwrap().unwrap();
        assert_eq!(f.consumed, n);
        assert_eq!(f.payload(&staging), b"hello world");
        assert_eq!(f.msg_type(&staging), b'h');
    }

    #[test]
    fn plain_empty_payload() {
        let mut out = [0u8; 64];
        let n = encode_plain(&[], &mut out).unwrap();
        let mut staging = [0u8; 64];
        let f = decode_plain(&out[..n], &mut staging).unwrap().unwrap();
        assert_eq!(f.payload_len, 0);
        assert_eq!(f.consumed, n);
    }

    #[test]
    fn plain_rejects_bad_length_and_pad() {
        let mut buf = [0u8; 64];
        let n = encode_plain(b"x", &mut buf).unwrap();
        // Corrupt length to a non-multiple of 8.
        let mut bad = buf;
        bad[0..4].copy_from_slice(&6u32.to_be_bytes());
        let mut staging = [0u8; 64];
        assert_eq!(
            decode_plain(&bad[..n], &mut staging),
            Err(DisconnectReason::ProtocolError)
        );
        // Corrupt padding_length to 0.
        let mut bad2 = buf;
        bad2[4] = 0;
        assert_eq!(
            decode_plain(&bad2[..n], &mut staging),
            Err(DisconnectReason::ProtocolError)
        );
        // Absurd length.
        let mut bad3 = buf;
        bad3[0..4].copy_from_slice(&(MAX_PACKET_LEN as u32 + 1).to_be_bytes());
        assert_eq!(
            decode_plain(&bad3[..4], &mut staging),
            Err(DisconnectReason::ProtocolError)
        );
        // Incomplete: only length field present, valid length -> None.
        assert!(decode_plain(&buf[..4], &mut staging).unwrap().is_none());
        assert!(decode_plain(&buf[..2], &mut staging).unwrap().is_none());
    }

    #[test]
    fn aead_roundtrip_and_seqno_nonce() {
        let k = keys(9);
        let mut scratch = [0u8; 128];
        let mut out = [0u8; 128];
        let n = encode_aead(b"data-1", &k, 0, &mut scratch, &mut out).unwrap();
        assert_eq!(n, out_len_bound(b"data-1"));
        let mut staging = [0u8; 128];
        let mut ls = [0u8; 4];
        let f = decode_aead(&out[..n], &mut staging, &k, 0, &mut ls)
            .unwrap()
            .unwrap();
        assert_eq!(f.payload(&staging), b"data-1");

        // Same plaintext at seqno 1 produces different ciphertext (nonce).
        let mut out2 = [0u8; 128];
        let n2 = encode_aead(b"data-1", &k, 1, &mut scratch, &mut out2).unwrap();
        assert_eq!(n2, n);
        assert_ne!(out[..n], out2[..n2]);
    }

    fn out_len_bound(payload: &[u8]) -> usize {
        4 + 1 + payload.len() + padding_len(payload.len(), BLOCK_SIZE) + 16
    }

    #[test]
    fn aead_wire_len_matches_formula() {
        let k = keys(3);
        let mut scratch = [0u8; 128];
        let mut out = [0u8; 128];
        let n = encode_aead(b"abc", &k, 5, &mut scratch, &mut out).unwrap();
        assert_eq!(n, out_len_bound(b"abc"));
    }

    #[test]
    fn aead_tamper_rejected() {
        let k = keys(11);
        let mut scratch = [0u8; 128];
        let mut out = [0u8; 128];
        let n = encode_aead(b"payload-7", &k, 5, &mut scratch, &mut out).unwrap();

        // Tamper with a payload ciphertext byte -> MacError.
        let mut bad = out;
        bad[8] ^= 1;
        let mut staging = [0u8; 128];
        let mut ls = [0u8; 4];
        assert_eq!(
            decode_aead(&bad[..n], &mut staging, &k, 5, &mut ls),
            Err(DisconnectReason::MacError)
        );

        // Tamper with the tag -> MacError.
        let mut bad2 = out;
        bad2[n - 1] ^= 0x80;
        assert_eq!(
            decode_aead(&bad2[..n], &mut staging, &k, 5, &mut ls),
            Err(DisconnectReason::MacError)
        );

        // Tamper with the encrypted length field so the decrypted length
        // lands out of bounds -> ProtocolError, never acceptance.
        let mut bad3 = out;
        bad3[1] ^= 0x40;
        assert_eq!(
            decode_aead(&bad3[..n], &mut staging, &k, 5, &mut ls),
            Err(DisconnectReason::ProtocolError)
        );

        // Wrong seqno (nonce mismatch) -> garbage length or mac failure.
        let r2 = decode_aead(&out[..n], &mut staging, &k, 6, &mut ls);
        assert!(r2.is_err());
    }

    #[test]
    fn aead_direction_isolation() {
        let c2s = keys(1);
        let s2c = keys(2);
        let mut scratch = [0u8; 128];
        let mut out = [0u8; 128];
        let n = encode_aead(b"secret", &c2s, 0, &mut scratch, &mut out).unwrap();
        let mut staging = [0u8; 128];
        let mut ls = [0u8; 4];
        // Wrong key yields a garbage (bounds-invalid) length or a MAC
        // failure — either way the packet is rejected.
        assert!(decode_aead(&out[..n], &mut staging, &s2c, 0, &mut ls).is_err());
    }

    #[test]
    fn aead_release_on_verify_only() {
        // On tamper the staging buffer must not receive plaintext.
        let k = keys(21);
        let mut scratch = [0u8; 128];
        let mut out = [0u8; 128];
        let n = encode_aead(b"do-not-release", &k, 2, &mut scratch, &mut out).unwrap();
        let mut bad = out;
        bad[10] ^= 0xff;
        let mut staging = [0x55u8; 128];
        let mut ls = [0u8; 4];
        let _ = decode_aead(&bad[..n], &mut staging, &k, 2, &mut ls);
        assert!(staging.iter().all(|&b| b == 0x55));
    }

    #[test]
    fn max_payload_boundary() {
        // Payload just under MAX_PAYLOAD_LEN encodes and round-trips.
        let payload = vec![0xABu8; MAX_PAYLOAD_LEN];
        let mut out = vec![0u8; MAX_WIRE_LEN];
        let n = encode_plain(&payload, &mut out).unwrap();
        let mut staging = vec![0u8; MAX_PACKET_LEN];
        let f = decode_plain(&out[..n], &mut staging).unwrap().unwrap();
        assert_eq!(f.payload_len, MAX_PAYLOAD_LEN);

        // One over the payload cap is refused.
        let too_big = vec![0u8; MAX_PAYLOAD_LEN + 1];
        assert_eq!(encode_plain(&too_big, &mut out), Err(WireErr::Overflow));
    }

    #[test]
    fn header_and_main_keys_are_distinct_slices() {
        let mut mat = [0u8; 64];
        for (i, b) in mat.iter_mut().enumerate() {
            *b = i as u8;
        }
        let k = CipherKeys::from_material(&mat);
        assert_eq!(k.main, mat[..32]);
        assert_eq!(k.header, mat[32..]);
    }

    #[test]
    fn aead_aad_is_encrypted_length() {
        // Flip ONLY the encrypted length bytes: MAC must fail even if the
        // (decrypted) length stays in bounds by luck.
        let k = keys(31);
        let mut scratch = [0u8; 128];
        let mut out = [0u8; 128];
        let n = encode_aead(b"z", &k, 1, &mut scratch, &mut out).unwrap();
        let mut staging = [0u8; 128];
        let mut ls = [0u8; 4];
        // Flip a length-ciphertext byte so the decrypted length stays in
        // bounds (12 ^ 8 = 4): the AAD no longer matches the tag check ->
        // MacError, proving the length field is authenticated.
        let mut bad = out;
        bad[3] ^= 0x08;
        assert_eq!(
            decode_aead(&bad[..n], &mut staging, &k, 1, &mut ls),
            Err(DisconnectReason::MacError)
        );
    }
}
