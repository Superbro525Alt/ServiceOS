//! `ssh-ed25519` public-key and signature blobs on the wire (RFC 4253 §6.6
//! + RFC 4253 §8 signature encoding).
//!
//! Trust honesty: this module verifies signatures cryptographically; it does
//! NOT authenticate host keys (no known_hosts store this wave). A valid
//! signature from an unrecognized key still passes — see the crate docs.

use crate::wire::{Reader, WireErr};
use serviceos_crypto::ed25519;

/// Host-key blob: `string "ssh-ed25519" | string pk(32)`.
pub fn host_key_blob(public_key: &[u8; 32], out: &mut [u8]) -> Result<usize, WireErr> {
    let mut w = crate::wire::Writer::new(out);
    w.string(b"ssh-ed25519")?;
    w.string(public_key)?;
    Ok(w.into_written())
}

/// Parse a host-key blob, returning the 32-byte ed25519 public key.
/// Rejects non-ssh-ed25519 algorithm names.
pub fn parse_host_key_blob(blob: &[u8]) -> Result<[u8; 32], WireErr> {
    let mut r = Reader::new(blob);
    let alg = r.string()?;
    if alg != b"ssh-ed25519" {
        return Err(WireErr::Truncated);
    }
    let pk = r.string()?;
    if pk.len() != 32 || r.remaining() != 0 {
        return Err(WireErr::Truncated);
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(pk);
    Ok(out)
}

/// Signature blob: `string "ssh-ed25519" | string sig(64)`.
pub fn signature_blob(signature: &[u8; 64], out: &mut [u8]) -> Result<usize, WireErr> {
    let mut w = crate::wire::Writer::new(out);
    w.string(b"ssh-ed25519")?;
    w.string(signature)?;
    Ok(w.into_written())
}

/// Parse a signature blob into the 64-byte ed25519 signature.
pub fn parse_signature_blob(blob: &[u8]) -> Result<[u8; 64], WireErr> {
    let mut r = Reader::new(blob);
    let alg = r.string()?;
    if alg != b"ssh-ed25519" {
        return Err(WireErr::Truncated);
    }
    let sig = r.string()?;
    if sig.len() != 64 || r.remaining() != 0 {
        return Err(WireErr::Truncated);
    }
    let mut out = [0u8; 64];
    out.copy_from_slice(sig);
    Ok(out)
}

/// Verify an exchange-hash signature against a parsed host key. Public-data
/// comparisons only — early rejection on malformed (public) shapes is fine.
pub fn verify_exchange_signature(
    public_key: &[u8; 32],
    exchange_hash: &[u8; 32],
    sig_blob: &[u8],
) -> Result<bool, WireErr> {
    let sig = parse_signature_blob(sig_blob)?;
    Ok(ed25519::verify(public_key, exchange_hash, &sig))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(n: u8) -> [u8; 32] {
        let mut s = [0u8; 32];
        s[0] = n;
        s[1] = 0x42;
        s[31] = n;
        s
    }

    #[test]
    fn blob_roundtrip() {
        let pk = ed25519::public_key(&seed(1));
        let mut buf = [0u8; 128];
        let n = host_key_blob(&pk, &mut buf).unwrap();
        let parsed = parse_host_key_blob(&buf[..n]).unwrap();
        assert_eq!(parsed, pk);
    }

    #[test]
    fn blob_rejects_wrong_alg_and_shape() {
        // ssh-rsa algorithm name in a blob.
        let mut buf = [0u8; 128];
        let mut w = crate::wire::Writer::new(&mut buf);
        w.string(b"ssh-rsa").unwrap();
        w.string(&[1u8; 32]).unwrap();
        let n = w.into_written();
        assert_eq!(parse_host_key_blob(&buf[..n]), Err(WireErr::Truncated));

        // Trailing junk.
        let pk = ed25519::public_key(&seed(2));
        let n = host_key_blob(&pk, &mut buf).unwrap();
        assert_eq!(parse_host_key_blob(&buf[..n + 1]), Err(WireErr::Truncated));
        // Wrong key length.
        let mut buf2 = [0u8; 128];
        let mut w2 = crate::wire::Writer::new(&mut buf2);
        w2.string(b"ssh-ed25519").unwrap();
        w2.string(&[1u8; 31]).unwrap();
        let n2 = w2.into_written();
        assert_eq!(parse_host_key_blob(&buf2[..n2]), Err(WireErr::Truncated));
    }

    #[test]
    fn signature_roundtrip_and_verify() {
        let sk = seed(3);
        let pk = ed25519::public_key(&sk);
        let mut msg = [0u8; 32];
        msg[..19].copy_from_slice(b"exchange-hash-bytes");
        let sig = ed25519::sign(&sk, &msg);
        let mut buf = [0u8; 128];
        let n = signature_blob(&sig, &mut buf).unwrap();
        assert!(verify_exchange_signature(&pk, &msg, &buf[..n]).unwrap());
        // Flipped hash bit -> verify fails (not an error).
        let mut h2 = msg;
        h2[0] ^= 1;
        assert!(!verify_exchange_signature(&pk, &h2, &buf[..n]).unwrap());
        // Wrong key -> false.
        let pk2 = ed25519::public_key(&seed(4));
        assert!(!verify_exchange_signature(&pk2, &msg, &buf[..n]).unwrap());
    }

    #[test]
    fn signature_blob_shape_rejected() {
        let mut buf = [0u8; 128];
        let mut w = crate::wire::Writer::new(&mut buf);
        w.string(b"rsa-sha2-256").unwrap();
        w.string(&[0u8; 64]).unwrap();
        let n = w.into_written();
        assert_eq!(parse_signature_blob(&buf[..n]), Err(WireErr::Truncated));

        let mut buf2 = [0u8; 128];
        let mut w2 = crate::wire::Writer::new(&mut buf2);
        w2.string(b"ssh-ed25519").unwrap();
        w2.string(&[0u8; 63]).unwrap();
        let n2 = w2.into_written();
        assert_eq!(parse_signature_blob(&buf2[..n2]), Err(WireErr::Truncated));
    }
}
