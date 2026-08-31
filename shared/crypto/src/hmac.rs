//! HMAC (RFC 2104) over SHA-512, pure-Rust, no_std.
//!
//! Incremental core (`HmacSha512`) plus a one-shot `hmac_sha512` convenience
//! matching the `parts`-slice shape previously used by the wireless platform
//! code, which now delegates here so exactly one HMAC implementation exists.

use crate::sha512::Sha512;

const BLOCK: usize = 128;

/// Incremental HMAC-SHA-512 (block size 128, RFC 2104).
pub struct HmacSha512 {
    inner: Sha512,
    opad_block: [u8; BLOCK],
}

impl HmacSha512 {
    pub fn new(key: &[u8]) -> HmacSha512 {
        let mut key_block = [0u8; BLOCK];
        if key.len() > BLOCK {
            // Long keys are hashed first (RFC 2104).
            let mut hash = Sha512::new();
            hash.update(key);
            key_block[..64].copy_from_slice(&hash.finalize());
        } else {
            key_block[..key.len()].copy_from_slice(key);
        }
        let mut ipad_block = [0x36u8; BLOCK];
        let mut opad_block = [0x5cu8; BLOCK];
        for index in 0..BLOCK {
            ipad_block[index] ^= key_block[index];
            opad_block[index] ^= key_block[index];
        }
        let mut inner = Sha512::new();
        inner.update(&ipad_block);
        HmacSha512 { inner, opad_block }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }

    pub fn finalize(self) -> [u8; 64] {
        let inner_digest = self.inner.finalize();
        let mut outer = Sha512::new();
        outer.update(&self.opad_block);
        outer.update(&inner_digest);
        outer.finalize()
    }
}

/// One-shot HMAC-SHA-512 over concatenated message parts.
pub fn hmac_sha512(key: &[u8], parts: &[&[u8]]) -> [u8; 64] {
    let mut mac = HmacSha512::new(key);
    for part in parts {
        mac.update(part);
    }
    mac.finalize()
}

#[cfg(test)]
mod tests_hmac {
    use super::*;

    /// RFC 4231 test case 1 (SHA-512 variant): key = 20×0x0b,
    /// data = "Hi There". RFC 4231 section 4.8 lists the SHA-512 digest.
    #[test]
    fn rfc4231_case1_sha512() {
        let key = [0x0bu8; 20];
        let mac = hmac_sha512(&key, &[b"Hi There"]);
        let expected: [u8; 64] = [
            0x87, 0xaa, 0x7c, 0xde, 0xa5, 0xef, 0x61, 0x9d, 0x4f, 0xf0, 0xb4, 0x24, 0x1a, 0x1d,
            0x6c, 0xb0, 0x23, 0x79, 0xf4, 0xe2, 0xce, 0x4e, 0xc2, 0x78, 0x7a, 0xd0, 0xb3, 0x05,
            0x45, 0xe1, 0x7c, 0xde, 0xda, 0xa8, 0x33, 0xb7, 0xd6, 0xb8, 0xa7, 0x02, 0x03, 0x8b,
            0x27, 0x4e, 0xae, 0xa3, 0xf4, 0xe4, 0xbe, 0x9d, 0x91, 0x4e, 0xeb, 0x61, 0xf1, 0x70,
            0x2e, 0x69, 0x6c, 0x20, 0x3a, 0x12, 0x68, 0x54,
        ];
        assert_eq!(mac, expected);
    }

    /// RFC 4231 test case 2 (SHA-512): key = "Jefe", data = "what do ya want
    /// for nothing?".
    #[test]
    fn rfc4231_case2_sha512() {
        let mac = hmac_sha512(b"Jefe", &[b"what do ya want for nothing?"]);
        let expected: [u8; 64] = [
            0x16, 0x4b, 0x7a, 0x7b, 0xfc, 0xf8, 0x19, 0xe2, 0xe3, 0x95, 0xfb, 0xe7, 0x3b, 0x56,
            0xe0, 0xa3, 0x87, 0xbd, 0x64, 0x22, 0x2e, 0x83, 0x1f, 0xd6, 0x10, 0x27, 0x0c, 0xd7,
            0xea, 0x25, 0x05, 0x54, 0x97, 0x58, 0xbf, 0x75, 0xc0, 0x5a, 0x99, 0x4a, 0x6d, 0x03,
            0x4f, 0x65, 0xf8, 0xf0, 0xe6, 0xfd, 0xca, 0xea, 0xb1, 0xa3, 0x4d, 0x4a, 0x6b, 0x4b,
            0x63, 0x6e, 0x07, 0x0a, 0x38, 0xbc, 0xe7, 0x37,
        ];
        assert_eq!(mac, expected);
    }

    /// Incremental feed must equal the one-shot digest (multi-part).
    #[test]
    fn incremental_matches_one_shot() {
        let key = [0x42u8; 77];
        let a = hmac_sha512(&key, &[b"Part One ", b"Part Two ", b"three"]);
        let mut mac = HmacSha512::new(&key);
        mac.update(b"Part One ");
        mac.update(b"Part Two ");
        mac.update(b"three");
        assert_eq!(mac.finalize(), a);
    }

    /// Long key (hashed first, RFC 2104) with an empty message; digest
    /// cross-verified against Python 3 `hmac.new(key, b"", sha512)`
    /// (OpenSSL). The empty-key digest is likewise cross-verified so the
    /// assertions are exact, not sanity checks.
    #[test]
    fn long_key_and_empty_edges() {
        let long_key = [0xaau8; 200];
        let expected: [u8; 64] = [
            0x76, 0x63, 0x6d, 0x1f, 0x09, 0x89, 0xc2, 0x47, 0x45, 0xa2, 0x36, 0xda, 0xee, 0x28,
            0x6a, 0xf9, 0xa6, 0x7e, 0x79, 0xa9, 0x5e, 0x3c, 0xc3, 0x8a, 0xae, 0x83, 0x1d, 0x1a,
            0x7a, 0x26, 0xe3, 0x59, 0x86, 0xf5, 0xf3, 0xbe, 0x42, 0x90, 0xc0, 0x1f, 0x3c, 0x4a,
            0xa7, 0xe0, 0x19, 0x73, 0xec, 0x58, 0x79, 0xf7, 0xab, 0x4a, 0xaf, 0xf1, 0x8b, 0x8f,
            0x3a, 0x94, 0x58, 0x97, 0xb9, 0x95, 0x79, 0xee,
        ];
        assert_eq!(hmac_sha512(&long_key, &[b""]), expected);

        let empty_key_expected: [u8; 64] = [
            0xae, 0xe6, 0x50, 0x45, 0x8d, 0xdc, 0xbc, 0xd4, 0x55, 0x75, 0x31, 0xb2, 0x7f, 0x1c,
            0xde, 0xd4, 0xc5, 0x77, 0xf5, 0x76, 0x07, 0x9b, 0x08, 0x6f, 0x57, 0xf3, 0xc6, 0xb9,
            0xe0, 0x37, 0xa7, 0xc0, 0xb3, 0xcd, 0xf7, 0xff, 0x58, 0xaf, 0xa0, 0x9f, 0xe2, 0x79,
            0x07, 0x6d, 0x11, 0x07, 0xd5, 0xd5, 0x54, 0xf4, 0xd3, 0x4f, 0xc6, 0xf0, 0x3d, 0x8f,
            0xd1, 0x2f, 0x93, 0xd8, 0x92, 0x80, 0x2a, 0xc3,
        ];
        assert_eq!(hmac_sha512(b"", &[b"x"]), empty_key_expected);
    }
}
