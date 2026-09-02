//! ChaCha20-Poly1305 AEAD (RFC 8439 §2.8), pure `core`, no heap.
//!
//! Caller-provided output buffers keep the API heap-free: `encrypt` writes
//! the ciphertext of `plaintext` into `ciphertext` and returns the tag;
//! `decrypt` verifies the tag (constant-time compare via `pbkdf2::ct_eq`)
//! and only then writes the plaintext, returning `false` — with the output
//! buffer untouched — on any failure.

use crate::chacha20;
use crate::pbkdf2::ct_eq;
use crate::poly1305::Poly1305;

pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 12;
pub const TAG_LEN: usize = 16;

/// Derive the one-time Poly1305 key: the first 32 bytes of the counter-0
/// ChaCha20 block (RFC 8439 §2.6).
fn poly_key_block(key: &[u8; KEY_LEN], nonce: &[u8; NONCE_LEN]) -> [u8; 32] {
    let block = chacha20::block(key, 0, nonce);
    let mut pk = [0u8; 32];
    pk.copy_from_slice(&block[..32]);
    pk
}

/// Authenticate `aad || ciphertext` with the AEAD length trailer
/// (RFC 8439 §2.8): each segment padded to 16, then le64 lengths.
fn compute_tag(
    key: &[u8; KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    aad: &[u8],
    ciphertext: &[u8],
) -> [u8; TAG_LEN] {
    let mut mac = Poly1305::new(&poly_key_block(key, nonce));
    mac.update(aad);
    mac.pad16();
    mac.update(ciphertext);
    mac.pad16();
    mac.update_le64(aad.len() as u64);
    mac.update_le64(ciphertext.len() as u64);
    mac.finalize()
}

/// Encrypt `plaintext` under (key, nonce) with associated data `aad`,
/// writing the ciphertext into `ciphertext` (same length) and returning the
/// 16-byte Poly1305 tag (RFC 8439 §2.8.1).
pub fn encrypt(
    key: &[u8; KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    aad: &[u8],
    plaintext: &[u8],
    ciphertext: &mut [u8],
) -> [u8; TAG_LEN] {
    debug_assert_eq!(ciphertext.len(), plaintext.len());
    // Message data uses counters 1..; counter 0 belongs to the Poly1305 key.
    chacha20::xor(key, 1, nonce, plaintext, ciphertext);
    compute_tag(key, nonce, aad, ciphertext)
}

/// Decrypt `ciphertext` under (key, nonce) with associated data `aad` and
/// tag `tag`, writing the plaintext into `plaintext` (same length).
/// Returns `true` on success; on tag mismatch the plaintext buffer is left
/// untouched and `false` is returned (RFC 8439 §2.8.2: release only on
/// verification success).
pub fn decrypt(
    key: &[u8; KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    aad: &[u8],
    ciphertext: &[u8],
    tag: &[u8; TAG_LEN],
    plaintext: &mut [u8],
) -> bool {
    debug_assert_eq!(plaintext.len(), ciphertext.len());
    let expect = compute_tag(key, nonce, aad, ciphertext);
    if !ct_eq(&expect, tag) {
        return false;
    }
    chacha20::xor(key, 1, nonce, ciphertext, plaintext);
    true
}

#[cfg(test)]
mod tests_chacha20poly1305 {
    use super::*;

    fn unhex<const N: usize>(s: &str) -> [u8; N] {
        let mut o = [0u8; N];
        for i in 0..N {
            o[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap();
        }
        o
    }

    const PLAINTEXT: &[u8] = b"Ladies and Gentlemen of the class of '99: If I could offer you \
only one tip for the future, sunscreen would be it.";

    /// RFC 8439 §2.8.2 (also Appendix A.4): full AEAD encryption vector —
    /// ciphertext and tag must match exactly, and decrypting inverts it.
    #[test]
    fn rfc8439_a_4_encrypt() {
        let key = unhex::<32>("808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f");
        let nonce = unhex::<12>("070000004041424344454647");
        let aad = unhex::<12>("50515253c0c1c2c3c4c5c6c7");
        let mut ct = [0u8; 114];
        let tag = encrypt(&key, &nonce, &aad, PLAINTEXT, &mut ct);
        assert_eq!(
            ct,
            unhex::<114>(
                "d31a8d34648e60db7b86afbc53ef7ec2a4aded51296e08fea9e2b5a736ee62d6\
                 3dbea45e8ca9671282fafb69da92728b1a71de0a9e060b2905d6a5b67ecd3b36\
                 92ddbd7f2d778b8c9803aee328091b58fab324e4fad675945585808b4831d7bc\
                 3ff4def08e4b7a9de576d26586cec64b6116"
            ),
            "ciphertext"
        );
        assert_eq!(tag, unhex::<16>("1ae10b594f09e26a7e902ecbd0600691"), "tag");

        let mut pt = [0u8; 114];
        assert!(decrypt(&key, &nonce, &aad, &ct, &tag, &mut pt));
        assert_eq!(pt, *PLAINTEXT);
    }

    /// Tag mismatch (flipped ciphertext byte) must fail without releasing
    /// plaintext; a correct message decrypts after the failure.
    #[test]
    fn tampered_ciphertext_fails_closed() {
        let key = unhex::<32>("808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f");
        let nonce = unhex::<12>("070000004041424344454647");
        let aad = unhex::<12>("50515253c0c1c2c3c4c5c6c7");
        let mut ct = [0u8; 114];
        let tag = encrypt(&key, &nonce, &aad, PLAINTEXT, &mut ct);

        let mut bad = ct;
        bad[7] ^= 0x01;
        let mut pt = [0xaau8; 114];
        assert!(!decrypt(&key, &nonce, &aad, &bad, &tag, &mut pt));
        assert!(pt.iter().all(|&b| b == 0xaa), "output untouched on failure");

        // Flipped AAD bit also fails.
        let mut bad_aad = aad;
        bad_aad[0] ^= 0x01;
        assert!(!decrypt(&key, &nonce, &bad_aad, &ct, &tag, &mut pt));

        // Flipped tag bit fails.
        let mut bad_tag = tag;
        bad_tag[15] ^= 0x80;
        assert!(!decrypt(&key, &nonce, &aad, &ct, &bad_tag, &mut pt));

        // Genuine message still decrypts after the failed attempts.
        assert!(decrypt(&key, &nonce, &aad, &ct, &tag, &mut pt));
        assert_eq!(pt, *PLAINTEXT);
    }

    /// Empty plaintext round-trips, both with and without AAD; AAD changes
    /// the tag even for an empty message.
    #[test]
    fn empty_inputs_roundtrip() {
        let key = [0x42u8; KEY_LEN];
        let nonce = [0x07u8; NONCE_LEN];
        let mut ct: [u8; 0] = [];
        let tag = encrypt(&key, &nonce, b"", b"", &mut ct);
        let mut pt: [u8; 0] = [];
        assert!(decrypt(&key, &nonce, b"", &ct, &tag, &mut pt));

        let tag2 = encrypt(&key, &nonce, b"associated", b"", &mut ct);
        assert_ne!(tag, tag2, "aad-only authentication must differ");
        assert!(decrypt(&key, &nonce, b"associated", &ct, &tag2, &mut pt));
        assert!(!decrypt(&key, &nonce, b"", &ct, &tag2, &mut pt));
    }
}
