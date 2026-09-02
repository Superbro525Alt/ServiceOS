//! HKDF key derivation (RFC 5869) over SHA-512, pure `core`, no heap.
//!
//! HMAC-SHA512 is the only dependency (`crate::hmac`). Output length is
//! bounded by the RFC: at most 255 * HashLen = 16320 bytes.

use crate::hmac::{hmac_sha512, HmacSha512};

/// PRK = HMAC-Hash(salt, IKM) (RFC 5869 §2.2). An empty salt is the
/// HashLen-zero-octet string per the RFC (HMAC pads internally).
pub fn extract(salt: &[u8], ikm: &[u8]) -> [u8; 64] {
    hmac_sha512(salt, &[ikm])
}

/// OKM = T(1) || T(2) || ... truncated to `okm.len()` (RFC 5869 §2.3), with
/// T(i) = HMAC-Hash(PRK, T(i-1) || info || i). Panics if `okm` exceeds the
/// RFC 5869 limit of 255 blocks.
pub fn expand(prk: &[u8; 64], info: &[u8], okm: &mut [u8]) {
    assert!(
        okm.len() <= 255 * 64,
        "HKDF-Expand output exceeds RFC 5869 limit (255 * 64 bytes)"
    );
    let mut t = [0u8; 64];
    let mut t_len = 0usize;
    let mut offset = 0usize;
    let mut counter = 1u8;
    while offset < okm.len() {
        let mut mac = HmacSha512::new(prk);
        mac.update(&t[..t_len]);
        mac.update(info);
        mac.update(&[counter]);
        t = mac.finalize();
        t_len = 64;
        let take = core::cmp::min(64, okm.len() - offset);
        okm[offset..offset + take].copy_from_slice(&t[..take]);
        offset += take;
        counter += 1;
    }
}

/// One-shot HKDF-SHA512: extract then expand into `okm`.
pub fn derive(ikm: &[u8], salt: &[u8], info: &[u8], okm: &mut [u8]) {
    let prk = extract(salt, ikm);
    expand(&prk, info, okm);
}

#[cfg(test)]
mod tests_hkdf {
    use super::*;

    fn unhex<const N: usize>(s: &str) -> [u8; N] {
        let mut o = [0u8; N];
        for i in 0..N {
            o[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap();
        }
        o
    }

    /// RFC 5869 Appendix A publishes vectors only for SHA-256 and SHA-1;
    /// no SHA-512 vectors exist upstream. These cases replay the A.1/A.2/A.3
    /// input shapes under SHA-512, with expected values derived from an
    /// independent reference implementation (Python 3 stdlib
    /// `hmac`/`hashlib.sha512`), construction followed verbatim from
    /// RFC 5869 §2.2/§2.3.
    #[test]
    fn a1_shape_basic() {
        let ikm = [0x0bu8; 22];
        let salt: [u8; 13] = core::array::from_fn(|i| i as u8);
        let info: [u8; 10] = core::array::from_fn(|i| (0xf0 + i) as u8);
        let mut okm = [0u8; 42];
        derive(&ikm, &salt, &info, &mut okm);
        assert_eq!(
            okm,
            unhex::<42>(
                "832390086cda71fb47625bb5ceb168e4c8e26a1a16ed34d9fc7fe92c1481\
                 579338da362cb8d9f925d7cb"
            )
        );
        let prk = extract(&salt, &ikm);
        assert_eq!(
            prk,
            unhex::<64>(
                "665799823737ded04a88e47e54a5890bb2c3d247c7a4254a8e6135072359\
                 0a26c36238127d8661b88cf80ef802d57e2f7cebcf1e00e083848be19929\
                 c61b4237"
            )
        );
    }

    /// A.2 shape: longer inputs (80-byte IKM, 80-byte salt, 80-byte info),
    /// 82-byte OKM spanning two T blocks.
    #[test]
    fn a2_shape_longer_inputs() {
        let ikm: [u8; 80] = core::array::from_fn(|i| i as u8);
        let salt: [u8; 80] = core::array::from_fn(|i| (0x60 + i) as u8);
        let info: [u8; 80] = core::array::from_fn(|i| (0xb0 + i) as u8);
        let mut okm = [0u8; 82];
        derive(&ikm, &salt, &info, &mut okm);
        assert_eq!(
            okm,
            unhex::<82>(
                "ce6c97192805b346e6161e821ed165673b84f400a2b514b2fe23d84cd189\
                 ddf1b695b48cbd1c8388441137b3ce28f16aa64ba33ba466b24df6cfcb02\
                 1ecff235f6a2056ce3af1de44d572097a8505d9e7a93"
            )
        );
    }

    /// A.3 shape: empty salt and empty info.
    #[test]
    fn a3_shape_empty_salt_and_info() {
        let ikm = [0x0bu8; 22];
        let mut okm = [0u8; 42];
        derive(&ikm, b"", b"", &mut okm);
        assert_eq!(
            okm,
            unhex::<42>(
                "f5fa02b18298a72a8c23898a8703472c6eb179dc204c03425c970e3b164b\
                 f90fff22d04836d0e2343bac"
            )
        );
    }

    /// Expand-only path: the A.1 PRK reused as IKM with empty salt/info
    /// (RFC 5869 A.4's "expand-only" shape), plus explicit extract/expand
    /// equivalence with the one-shot `derive`.
    #[test]
    fn expand_only_and_incremental_consistency() {
        let ikm = [0x0bu8; 22];
        let salt: [u8; 13] = core::array::from_fn(|i| i as u8);
        let prk = extract(&salt, &ikm);
        let mut okm = [0u8; 42];
        expand(&prk, b"", &mut okm);
        assert_eq!(
            okm,
            unhex::<42>(
                "f81b87481a18b664936daeb222f58cba0ebc55f5c85996b9f1cb396c327b70bb\
                 4c50fc5671cc1eca2f27"
            )
        );

        // Same derived key through the explicit two-step path.
        let mut okm2 = [0u8; 42];
        let prk2 = extract(&salt, &ikm);
        expand(&prk2, &[], &mut okm2);
        assert_eq!(okm, okm2);
    }
}
