//! PBKDF2 (RFC 2898 §5.2 / RFC 8018 §5.2) over HMAC-SHA-512, pure-Rust,
//! no_std. Fixed 64-byte output (one PBKDF2 block) with a constant-time
//! comparison helper for password verification.

use crate::hmac::HmacSha512;

/// Derive 64 bytes from `password` and `salt` with `iterations` PBKDF2-HMAC-
/// SHA-512 rounds (dkLen = 64, exactly one derived block, so U_1 is
/// HMAC(P, salt || INT(1)) and U_i = HMAC(P, U_{i-1})).
///
/// `iterations == 0` is rejected (RFC 8018 requires c >= 1) by writing the
/// zero digest; callers that need a fallible signal should check their own
/// inputs. Panics never: all buffers are fixed-size.
pub fn pbkdf2_hmac_sha512(password: &[u8], salt: &[u8], iterations: u32, out: &mut [u8; 64]) {
    if iterations == 0 {
        *out = [0u8; 64];
        return;
    }
    // U_1: HMAC over salt || INT(1); fed incrementally so salts of any
    // length work without allocation.
    let mut u = {
        let mut mac = HmacSha512::new(password);
        mac.update(salt);
        mac.update(&[0, 0, 0, 1]);
        mac.finalize()
    };
    let mut accumulated: [u8; 64] = u;
    let rounds = iterations - 1;
    for _ in 0..rounds {
        // U_i = HMAC(P, U_{i-1}).
        let mut mac = HmacSha512::new(password);
        mac.update(&u);
        u = mac.finalize();
        for index in 0..64 {
            accumulated[index] ^= u[index];
        }
    }
    *out = accumulated;
}

/// Constant-time equality over byte slices of equal declared length: walks
/// every byte regardless of where (or whether) a mismatch occurs and folds
/// differences into an OR accumulator. Length mismatch returns false upfront
/// (lengths are not secret here).
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for index in 0..a.len() {
        diff |= a[index] ^ b[index];
    }
    diff == 0
}

#[cfg(test)]
mod tests_pbkdf2 {
    use super::*;

    /// Decode 64 hex chars into bytes for vector comparisons.
    fn hex32(text: &str) -> [u8; 32] {
        assert_eq!(text.len(), 64);
        let mut out = [0u8; 32];
        for index in 0..32 {
            let high = (text.as_bytes()[index * 2] as char).to_digit(16).unwrap() as u8;
            let low = (text.as_bytes()[index * 2 + 1] as char)
                .to_digit(16)
                .unwrap() as u8;
            out[index] = (high << 4) | low;
        }
        out
    }

    /// Published PBKDF2-HMAC-SHA512 vectors (the widely mirrored "SHA-512
    /// adaptation" of the RFC 6070 PBKDF2-HMAC-SHA1 set, as used by hashlib
    /// cross-checks and e.g. the Go/Rust PBKDF2 test suites). Each vector was
    /// additionally cross-verified against Python 3
    /// `hashlib.pbkdf2_hmac('sha512', ...)` (OpenSSL) for this crate.
    #[test]
    fn published_vectors_c1_c2_c4096() {
        let mut out = [0u8; 64];

        // P="password", S="salt", c=1.
        pbkdf2_hmac_sha512(b"password", b"salt", 1, &mut out);
        assert_eq!(
            &out[..32],
            &hex32("867f70cf1ade02cff3752599a3a53dc4af34c7a669815ae5d513554e1c8cf252")
        );

        // P="password", S="salt", c=2.
        pbkdf2_hmac_sha512(b"password", b"salt", 2, &mut out);
        assert_eq!(
            &out[..32],
            &hex32("e1d9c16aa681708a45f5c7c4e215ceb66e011a2e9f0040713f18aefdb866d53c")
        );

        // P="password", S="salt", c=4096.
        pbkdf2_hmac_sha512(b"password", b"salt", 4096, &mut out);
        assert_eq!(
            &out[..32],
            &hex32("d197b1b33db0143e018b12f3d1d1479e6cdebdcc97c5c0f87f6902e072f457b5")
        );
    }

    /// Published long-password / long-salt vector (c=4096, dkLen=100): block 1
    /// is independent of dkLen, so the first 32 bytes must match the first 32
    /// bytes of the published 100-byte digest.
    #[test]
    fn published_vector_long_password_long_salt() {
        let password = b"passwordPASSWORDpassword";
        let salt = b"saltSALTsaltSALTsaltSALTsaltSALTsalt";
        let mut out = [0u8; 64];
        pbkdf2_hmac_sha512(password, salt, 4096, &mut out);
        assert_eq!(
            &out[..32],
            &hex32("8c0511f4c6e597c6ac6315d8f0362e225f3c501495ba23b868c005174dc4ee71")
        );
    }

    /// Published c=80000 vector: P="Password", S="NaCl".
    #[test]
    fn published_vector_c80000() {
        let mut out = [0u8; 64];
        pbkdf2_hmac_sha512(b"Password", b"NaCl", 80000, &mut out);
        assert_eq!(
            &out[..32],
            &hex32("e6337d6fbeb645c794d4a9b5b75b7b30dac9ac50376a91df1f4460f6060d5add")
        );
    }

    /// Full 64-byte output for the c=1 vector (second 32 bytes too),
    /// cross-verified against Python 3 hashlib.pbkdf2_hmac.
    #[test]
    fn full_64_byte_output_c1() {
        let mut out = [0u8; 64];
        pbkdf2_hmac_sha512(b"password", b"salt", 1, &mut out);
        assert_eq!(
            &out[32..],
            &hex32("c02d470a285a0501bad999bfe943c08f050235d7d68b1da55e63f73b60a57fce")
        );
    }

    /// Edge behavior: empty password, empty salt, iterations = 1 vs 0.
    #[test]
    fn edge_cases_empty_inputs_and_zero_iterations() {
        let mut out = [0u8; 64];
        // Empty password and empty salt with c=1: U1 = HMAC(b"", INT(1)).
        // Digest cross-verified against Python 3 hashlib.pbkdf2_hmac.
        pbkdf2_hmac_sha512(b"", b"", 1, &mut out);
        assert_eq!(
            &out[..32],
            &hex32("6d2ecbbbfb2e6dcd7056faf9af6aa06eae594391db983279a6bf27e0eb228614")
        );

        // iterations == 0 writes the zero digest (documented rejection).
        pbkdf2_hmac_sha512(b"password", b"salt", 0, &mut out);
        assert_eq!(out, [0u8; 64]);

        // Different salts must produce different output even at c=1.
        pbkdf2_hmac_sha512(b"password", b"saltA", 1, &mut out);
        let mut out2 = [0u8; 64];
        pbkdf2_hmac_sha512(b"password", b"saltB", 1, &mut out2);
        assert_ne!(out, out2);
    }

    /// ct_eq: equal content true; any single-byte mismatch false; length
    /// mismatch false; every position of mismatch detected (exercises the
    /// late-difference path of the constant-time walk).
    #[test]
    fn ct_eq_behavior() {
        assert!(ct_eq(b"identical", b"identical"));
        assert!(ct_eq(&[], &[]));
        assert!(!ct_eq(b"identical", b"identicalX"));
        for position in 0..8 {
            let mut flipped = *b"identical";
            flipped[position] ^= 0x01;
            assert!(!ct_eq(b"identical", &flipped), "mismatch at {}", position);
        }
        assert!(!ct_eq(&[0u8; 3], &[0u8, 0, 1]));
    }

    /// Determinism and password sensitivity of the KDF.
    #[test]
    fn deterministic_and_password_sensitive() {
        let mut first = [0u8; 64];
        let mut again = [0u8; 64];
        let mut other = [0u8; 64];
        pbkdf2_hmac_sha512(b"hunter2", b"pepper", 64, &mut first);
        pbkdf2_hmac_sha512(b"hunter2", b"pepper", 64, &mut again);
        pbkdf2_hmac_sha512(b"hunter3", b"pepper", 64, &mut other);
        assert_eq!(first, again);
        assert_ne!(first, other);
        assert!(ct_eq(&first, &again));
        assert!(!ct_eq(&first, &other));
    }
}
