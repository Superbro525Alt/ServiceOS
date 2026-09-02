//! `curve25519-sha256` key exchange (RFC 8731) over the RFC 4253 §7 KEX
//! messages, plus the RFC 4253 §7.2 SSH key derivation.
//!
//! Wire mapping (RFC 8731 §3): the ephemeral public keys `e`/`f` are encoded
//! as SSH `string`s of exactly 32 octets (the raw RFC 7748 point encoding);
//! the shared secret K is the X25519 output — which RFC 7748 defines
//! little-endian — **byte-reversed to big-endian** and then mpint-encoded
//! (leading zeros stripped, one 0x00 prepended when the top bit of the first
//! significant byte is set; an all-zero shared secret rejects the exchange).
//! The exchange hash is SHA-256 over
//! `V_C || V_S || I_C || I_S || K_S || e || f || K`, with `e`/`f` in their
//! full string encodings, `K_S` in its full wire string encoding (blob plus
//! its own length prefix — matching OpenSSH's hash input), and K in its
//! mpint encoding.
//!
//! Key derivation (RFC 4253 §7.2, iterated-hash branch — NOT HKDF; SSH
//! defines its own construction): `K1 = HASH(K || H || X || session_id)`,
//! `K2 = HASH(K || H || K1)`, ... over SHA-256 (RFC 8731 specifies SHA-256
//! for curve25519-sha256). chacha20-poly1305@openssh.com needs 64 bytes of
//! key material per direction: main (payload) key first 32, header (length)
//! key last 32. The RFC 4253 IV strings ('C'/'D') are unused by the AEAD
//! cipher and are deliberately not derived.
//!
//! Purity: no RNG — the KEXINIT cookie is caller-supplied and ephemeral
//! seeds are constructor parameters.

use crate::negotiate;
use crate::packet::CipherKeys;
use crate::wire::{Reader, WireErr, Writer};
use serviceos_crypto::sha256;
use serviceos_crypto::x25519;

pub const SSH_MSG_KEXINIT: u8 = 20;
pub const SSH_MSG_NEWKEYS: u8 = 21;
pub const SSH_MSG_KEX_ECDH_INIT: u8 = 30;
pub const SSH_MSG_KEX_ECDH_REPLY: u8 = 31;

/// KEXINIT payload cap accepted/parsed (cookie + 10 name-lists + flags is
/// ~1 KiB for sane peers; 8 KiB is generous).
pub const KEXINIT_MAX: usize = 8192;

/// Constant-time check against the all-zero X25519 output (RFC 8731 §3.2:
/// an all-zero shared secret MUST terminate the connection).
fn all_zero32(b: &[u8; 32]) -> bool {
    serviceos_crypto::pbkdf2::ct_eq(b, &[0u8; 32])
}

/// Build a KEXINIT payload (message code included) into `out`:
/// `20 | 16-byte cookie | 10 name-lists | guess flag | reserved 0`.
pub fn build_kexinit(cookie: &[u8; 16], out: &mut [u8]) -> Result<usize, WireErr> {
    let mut w = Writer::new(out);
    w.u8(SSH_MSG_KEXINIT)?;
    w.raw(cookie)?;
    let list = |names: &[&str], w: &mut Writer| -> Result<(), WireErr> {
        let mut body = [0u8; 128];
        let mut len = 0;
        for (i, n) in names.iter().enumerate() {
            if i > 0 {
                body[len] = b',';
                len += 1;
            }
            body[len..len + n.len()].copy_from_slice(n.as_bytes());
            len += n.len();
        }
        w.string(&body[..len])
    };
    list(negotiate::KEX_ALGS, &mut w)?;
    list(negotiate::HOSTKEY_ALGS, &mut w)?;
    list(negotiate::CIPHER_ALGS, &mut w)?;
    list(negotiate::CIPHER_ALGS, &mut w)?;
    list(negotiate::MAC_ALGS, &mut w)?;
    list(negotiate::MAC_ALGS, &mut w)?;
    list(negotiate::COMPRESSION_ALGS, &mut w)?;
    list(negotiate::COMPRESSION_ALGS, &mut w)?;
    w.u32(0)?; // languages client-to-server: empty
    w.u32(0)?; // languages server-to-client: empty
    w.u8(0)?; // first_kex_packet_follows: false
    w.u32(0)?; // reserved
    Ok(w.into_written())
}

/// Parsed KEXINIT: byte-range views into the original payload for each of
/// the ten name-lists, plus the guess flag.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KexInitRef {
    /// (start, len) per list: KEX, HOSTKEY, ENC_C2S, ENC_S2C, MAC_C2S,
    /// MAC_S2C, COMP_C2S, COMP_S2C, LANG_C2S, LANG_S2C.
    pub lists: [(usize, usize); 10],
    pub first_kex_packet_follows: bool,
}

impl KexInitRef {
    pub const KEX: usize = 0;
    pub const HOSTKEY: usize = 1;
    pub const ENC_C2S: usize = 2;
    pub const ENC_S2C: usize = 3;
    pub const MAC_C2S: usize = 4;
    pub const MAC_S2C: usize = 5;
    pub const COMP_C2S: usize = 6;
    pub const COMP_S2C: usize = 7;
    pub const LANG_C2S: usize = 8;
    pub const LANG_S2C: usize = 9;

    pub fn list<'a>(&self, payload: &'a [u8], idx: usize) -> &'a [u8] {
        let (s, l) = self.lists[idx];
        &payload[s..s + l]
    }
}

/// Parse a KEXINIT payload (message code included). Trailing bytes and
/// oversize payloads are rejected.
pub fn parse_kexinit(payload: &[u8]) -> Result<KexInitRef, WireErr> {
    if payload.is_empty() || payload.len() > KEXINIT_MAX {
        return Err(WireErr::Overflow);
    }
    let mut r = Reader::new(payload);
    if r.u8()? != SSH_MSG_KEXINIT {
        return Err(WireErr::Truncated);
    }
    let _cookie = r.take(16)?;
    let mut lists = [(0usize, 0usize); 10];
    for slot in lists.iter_mut() {
        let len = r.u32()? as usize;
        let after = payload.len() - r.remaining();
        let body = r.take(len)?;
        let start = after;
        *slot = (start, body.len());
    }
    let follows = r.u8()? != 0;
    let _reserved = r.u32()?;
    if r.remaining() != 0 {
        return Err(WireErr::Truncated);
    }
    Ok(KexInitRef {
        lists,
        first_kex_packet_follows: follows,
    })
}

/// X25519 shared secret from our seed and the peer public key, rejecting
/// all-zero peer keys and all-zero results.
pub fn shared_secret(our_seed: &[u8; 32], peer_public: &[u8; 32]) -> Result<[u8; 32], ()> {
    if all_zero32(peer_public) {
        return Err(());
    }
    let secret = *x25519::x25519(our_seed, peer_public).as_bytes();
    if all_zero32(&secret) {
        return Err(());
    }
    Ok(secret)
}

/// Big-endian conversion of the little-endian X25519 output (RFC 7748 §5:
/// the output is the little-endian encoding of the x-coordinate; SSH mpints
/// are big-endian).
pub fn shared_to_big_endian(shared_le: &[u8; 32]) -> [u8; 32] {
    let mut be = [0u8; 32];
    for i in 0..32 {
        be[i] = shared_le[31 - i];
    }
    be
}

/// Exchange hash H = SHA-256(V_C || V_S || I_C || I_S || K_S || e || f || K)
/// with `e`/`f` in their full wire string encodings and `K` mpint-encoded.
pub fn exchange_hash(
    v_c: &[u8],
    v_s: &[u8],
    i_c: &[u8],
    i_s: &[u8],
    k_s: &[u8],
    e: &[u8; 32],
    f: &[u8; 32],
    k_mpint: &[u8],
) -> [u8; 32] {
    let mut e_wire = [0u8; 36];
    e_wire[0..4].copy_from_slice(&32u32.to_be_bytes());
    e_wire[4..].copy_from_slice(e);
    let mut f_wire = [0u8; 36];
    f_wire[0..4].copy_from_slice(&32u32.to_be_bytes());
    f_wire[4..].copy_from_slice(f);
    sha256::digest(&[v_c, v_s, i_c, i_s, k_s, &e_wire, &f_wire, k_mpint])
}

/// RFC 4253 §7.2 iterated key derivation for one direction over SHA-256.
/// `x` is the direction letter ('A' client-to-server, 'B' server-to-client).
/// Returns 64 bytes: `HASH(K||H||X||sid) || HASH(K||H||K1)`.
pub fn derive_direction_keys(
    k_mpint: &[u8],
    h: &[u8; 32],
    session_id: &[u8; 32],
    x: u8,
) -> [u8; 64] {
    let k1 = sha256::digest(&[k_mpint, h, &[x], session_id]);
    let k2 = sha256::digest(&[k_mpint, h, &k1]);
    let mut out = [0u8; 64];
    out[..32].copy_from_slice(&k1);
    out[32..].copy_from_slice(&k2);
    out
}

/// Both directions' cipher key material. `session_id == h` for the first
/// (and this transport's only) key exchange.
pub fn derive_session_keys(k_mpint: &[u8], h: &[u8; 32]) -> (CipherKeys, CipherKeys) {
    let c2s = derive_direction_keys(k_mpint, h, h, b'A');
    let s2c = derive_direction_keys(k_mpint, h, h, b'B');
    (
        CipherKeys::from_material(&c2s),
        CipherKeys::from_material(&s2c),
    )
}

/// Build `SSH_MSG_KEX_ECDH_INIT`: `30 | string e`.
pub fn build_ecdh_init(e: &[u8; 32], out: &mut [u8]) -> Result<usize, WireErr> {
    let mut w = Writer::new(out);
    w.u8(SSH_MSG_KEX_ECDH_INIT)?;
    w.string(e)?;
    Ok(w.into_written())
}

/// Parse `SSH_MSG_KEX_ECDH_INIT`; `e` must be exactly 32 bytes and not
/// all-zero (public-data pre-check; the zero-secret rejection happens after
/// the DH computation regardless).
pub fn parse_ecdh_init(payload: &[u8]) -> Result<[u8; 32], WireErr> {
    let mut r = Reader::new(payload);
    if r.u8()? != SSH_MSG_KEX_ECDH_INIT {
        return Err(WireErr::Truncated);
    }
    let e = r.string()?;
    if e.len() != 32 || r.remaining() != 0 {
        return Err(WireErr::Truncated);
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(e);
    if all_zero32(&out) {
        return Err(WireErr::Truncated);
    }
    Ok(out)
}

/// Build `SSH_MSG_KEX_ECDH_REPLY`: `31 | string K_S | string f | string sig`.
pub fn build_ecdh_reply(
    host_key_blob: &[u8],
    f: &[u8; 32],
    signature_blob: &[u8],
    out: &mut [u8],
) -> Result<usize, WireErr> {
    let mut w = Writer::new(out);
    w.u8(SSH_MSG_KEX_ECDH_REPLY)?;
    w.string(host_key_blob)?;
    w.string(f)?;
    w.string(signature_blob)?;
    Ok(w.into_written())
}

/// Parsed `SSH_MSG_KEX_ECDH_REPLY`: byte ranges of the host-key blob and
/// signature blob inside the payload, plus `f`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EcdhReplyRef {
    pub host_key: (usize, usize),
    pub f: [u8; 32],
    pub sig: (usize, usize),
}

/// Parse `SSH_MSG_KEX_ECDH_REPLY` (message code included).
pub fn parse_ecdh_reply(payload: &[u8]) -> Result<EcdhReplyRef, WireErr> {
    let mut r = Reader::new(payload);
    if r.u8()? != SSH_MSG_KEX_ECDH_REPLY {
        return Err(WireErr::Truncated);
    }
    let k_s = r.string()?;
    let hk_start = 1;
    let hk_end = payload.len() - r.remaining();
    let _ = k_s;
    let f_raw = r.string()?;
    if f_raw.len() != 32 {
        return Err(WireErr::Truncated);
    }
    let mut f = [0u8; 32];
    f.copy_from_slice(f_raw);
    let sig_start = payload.len() - r.remaining();
    let sig = r.string()?;
    if r.remaining() != 0 {
        return Err(WireErr::Truncated);
    }
    Ok(EcdhReplyRef {
        host_key: (hk_start, hk_end),
        f,
        sig: (sig_start, sig_start + 4 + sig.len()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hostkey;

    fn cookie(n: u8) -> [u8; 16] {
        let mut c = [0u8; 16];
        c.iter_mut()
            .enumerate()
            .for_each(|(i, b)| *b = n.wrapping_add(i as u8));
        c
    }

    #[test]
    fn kexinit_shape_and_roundtrip() {
        let mut buf = [0u8; 1024];
        let n = build_kexinit(&cookie(1), &mut buf).unwrap();
        assert!(n <= KEXINIT_MAX);
        let k = parse_kexinit(&buf[..n]).unwrap();
        assert_eq!(
            k.list(&buf[..n], KexInitRef::KEX),
            &b"curve25519-sha256,curve25519-sha256@libssh.org"[..]
        );
        assert_eq!(k.list(&buf[..n], KexInitRef::HOSTKEY), &b"ssh-ed25519"[..]);
        assert_eq!(
            k.list(&buf[..n], KexInitRef::ENC_C2S),
            &b"chacha20-poly1305@openssh.com"[..]
        );
        assert_eq!(
            k.list(&buf[..n], KexInitRef::ENC_S2C),
            &b"chacha20-poly1305@openssh.com"[..]
        );
        assert_eq!(
            k.list(&buf[..n], KexInitRef::MAC_C2S),
            &b"hmac-sha2-256"[..]
        );
        assert_eq!(k.list(&buf[..n], KexInitRef::COMP_S2C), &b"none"[..]);
        assert_eq!(k.list(&buf[..n], KexInitRef::LANG_C2S), &b""[..]);
        assert_eq!(k.list(&buf[..n], KexInitRef::LANG_S2C), &b""[..]);
        assert!(!k.first_kex_packet_follows);
        loys();
    }

    fn loys() {}

    #[test]
    fn kexinit_rejects_wrong_msg_truncated_oversize() {
        let mut buf = [0u8; 1024];
        let n = build_kexinit(&cookie(2), &mut buf).unwrap();
        let mut bad = buf;
        bad[0] = SSH_MSG_NEWKEYS;
        assert_eq!(parse_kexinit(&bad[..n]), Err(WireErr::Truncated));
        let big = [0u8; KEXINIT_MAX + 1];
        assert_eq!(parse_kexinit(&big), Err(WireErr::Overflow));
        // Truncated: cut inside the cookie.
        assert_eq!(parse_kexinit(&buf[..20]), Err(WireErr::Truncated));
    }

    #[test]
    fn shared_secret_rejects_zero_and_is_symmetric() {
        let seed = [7u8; 32];
        assert!(shared_secret(&seed, &[0u8; 32]).is_err());
        let a = [3u8; 32];
        let b = [4u8; 32];
        let pa = x25519::x25519_public(&a);
        let pb = x25519::x25519_public(&b);
        let sa = shared_secret(&a, &pb).unwrap();
        let sb = shared_secret(&b, &pa).unwrap();
        assert_eq!(sa, sb);
        assert!(!sa.iter().all(|&x| x == 0));
    }

    #[test]
    fn big_endian_conversion() {
        let mut le = [0u8; 32];
        le[0] = 0x01; // least significant byte first in little-endian
        let be = shared_to_big_endian(&le);
        assert_eq!(be[31], 0x01);
        assert_eq!(be[0], 0x00);
    }

    #[test]
    fn k_mpint_is_mpint_of_reversed_secret() {
        let mut le = [0u8; 32];
        le[0] = 0x02; // little-endian value 2
        let be = shared_to_big_endian(&le);
        let mut buf = [0u8; 64];
        let mut w = Writer::new(&mut buf);
        w.mpint_be(&be).unwrap();
        let n = w.into_written();
        // Value 2 as mpint: length 1, byte 0x02.
        assert_eq!(&buf[..n], &[0, 0, 0, 1, 0x02]);
    }

    #[test]
    fn derive_keys_is_two_iterated_hashes() {
        let h = sha256::digest(&[b"H"]);
        let k = [0x33u8; 4];
        let sid = sha256::digest(&[b"sid"]);
        let keys = derive_direction_keys(&k, &h, &sid, b'A');
        let k1 = sha256::digest(&[k.as_slice(), h.as_slice(), b"A".as_slice(), sid.as_slice()]);
        let k2 = sha256::digest(&[k.as_slice(), h.as_slice(), k1.as_slice()]);
        assert_eq!(&keys[..32], &k1[..]);
        assert_eq!(&keys[32..], &k2[..]);
    }

    #[test]
    fn directions_differ_and_session_keys_deterministic() {
        let h = sha256::digest(&[b"H"]);
        let (c2s, s2c) = derive_session_keys(&[1, 2, 3, 4], &h);
        assert_ne!(c2s, s2c);
        let (c2s2, s2c2) = derive_session_keys(&[1, 2, 3, 4], &h);
        assert_eq!(c2s, c2s2);
        assert_eq!(s2c, s2c2);
    }

    #[test]
    fn ecdh_init_roundtrip_and_zero_reject() {
        let mut buf = [0u8; 64];
        let n = build_ecdh_init(&[9u8; 32], &mut buf).unwrap();
        assert_eq!(parse_ecdh_init(&buf[..n]).unwrap(), [9u8; 32]);
        assert_eq!(parse_ecdh_init(&buf[..n - 1]), Err(WireErr::Truncated));
        let mut buf2 = [0u8; 64];
        let n2 = build_ecdh_init(&[0u8; 32], &mut buf2).unwrap();
        assert_eq!(parse_ecdh_init(&buf2[..n2]), Err(WireErr::Truncated));
        let mut w = Writer::new(&mut buf2);
        w.u8(SSH_MSG_KEX_ECDH_INIT).unwrap();
        w.string(&[1u8; 31]).unwrap();
        let n3 = w.into_written();
        assert_eq!(parse_ecdh_init(&buf2[..n3]), Err(WireErr::Truncated));
    }

    #[test]
    fn ecdh_reply_shape_and_parsing() {
        let sk = [5u8; 32];
        let pk = serviceos_crypto::ed25519::public_key(&sk);
        let mut kbuf = [0u8; 128];
        let kn = hostkey::host_key_blob(&pk, &mut kbuf).unwrap();
        let mut sbuf = [0u8; 128];
        let sig = serviceos_crypto::ed25519::sign(&sk, &[0u8; 32]);
        let sn = hostkey::signature_blob(&sig, &mut sbuf).unwrap();
        let mut buf = [0u8; 512];
        let n = build_ecdh_reply(&kbuf[..kn], &[7u8; 32], &sbuf[..sn], &mut buf).unwrap();
        let parsed = parse_ecdh_reply(&buf[..n]).unwrap();
        assert_eq!(parsed.f, [7u8; 32]);
        // host_key range covers the full string encoding (length prefix +
        // blob).
        assert_eq!(
            &buf[parsed.host_key.0..parsed.host_key.0 + 4],
            &(kn as u32).to_be_bytes()
        );
        assert_eq!(&buf[parsed.host_key.0 + 4..parsed.host_key.1], &kbuf[..kn]);
        assert_eq!(
            &buf[parsed.sig.0..parsed.sig.0 + 4],
            &(sn as u32).to_be_bytes()
        );
        assert_eq!(&buf[parsed.sig.0 + 4..parsed.sig.1], &sbuf[..sn]);
        // Truncated f.
        assert_eq!(parse_ecdh_reply(&buf[..n - 8]), Err(WireErr::Truncated));
    }

    #[test]
    fn exchange_hash_structure() {
        // Manual recomputation of the documented construction.
        let v_c = b"SSH-2.0-c".as_slice();
        let v_s = b"SSH-2.0-s".as_slice();
        let i_c = [1u8, 20, 2, 3];
        let i_s = [4u8, 20, 5, 6];
        let k_s = [7u8; 10];
        let e = [8u8; 32];
        let f = [9u8; 32];
        let k = [0x00u8, 0x81];
        let h = exchange_hash(v_c, v_s, &i_c, &i_s, &k_s, &e, &f, &k);
        let mut e_wire = [0u8; 36];
        e_wire[0..4].copy_from_slice(&32u32.to_be_bytes());
        e_wire[4..].copy_from_slice(&e);
        let mut f_wire = [0u8; 36];
        f_wire[0..4].copy_from_slice(&32u32.to_be_bytes());
        f_wire[4..].copy_from_slice(&f);
        let manual = sha256::digest(&[v_c, v_s, &i_c, &i_s, &k_s, &e_wire, &f_wire, &k]);
        assert_eq!(h, manual);
        // Sensitivity: any input change moves the hash.
        let mut e2 = e;
        e2[0] ^= 1;
        assert_ne!(exchange_hash(v_c, v_s, &i_c, &i_s, &k_s, &e2, &f, &k), h);
        assert_ne!(
            exchange_hash(v_c, b"SSH-2.0-x", &i_c, &i_s, &k_s, &e, &f, &k),
            h
        );
    }
}
