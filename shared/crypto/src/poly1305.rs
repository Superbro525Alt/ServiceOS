//! Poly1305 one-time authenticator (RFC 8439 §2.5), pure `core`, no heap.
//!
//! Five 26-bit limbs with u64 products (the classic "donna" formulation).
//! Limb arithmetic is constant-time by construction; the final reduction
//! uses masked selects rather than branches. Incremental API so AEAD can
//! stream AAD / ciphertext / length words without concatenating buffers.

const MASK26: u32 = (1 << 26) - 1;

/// Incremental Poly1305. One instance authenticates exactly one message
/// (the key is a one-time pad — RFC 8439 §2.5).
pub struct Poly1305 {
    r: [u32; 5],
    h: [u32; 5],
    pad: [u32; 4],
    buf: [u8; 16],
    buf_len: usize,
}

impl Poly1305 {
    /// Initialize from the 32-byte one-time key: r = clamp(key[0..16]),
    /// s = key[16..32] (RFC 8439 §2.5).
    pub fn new(key: &[u8; 32]) -> Poly1305 {
        let le32 = |b: &[u8]| u32::from_le_bytes(b.try_into().unwrap());
        let r = [
            le32(&key[0..4]) & 0x3ff_ffff,
            (le32(&key[3..7]) >> 2) & 0x3ff_ff03,
            (le32(&key[6..10]) >> 4) & 0x3ff_c0ff,
            (le32(&key[9..13]) >> 6) & 0x3f0_3fff,
            (le32(&key[12..16]) >> 8) & 0x00f_ffff,
        ];
        let pad = [
            le32(&key[16..20]),
            le32(&key[20..24]),
            le32(&key[24..28]),
            le32(&key[28..32]),
        ];
        Poly1305 {
            r,
            h: [0; 5],
            pad,
            buf: [0; 16],
            buf_len: 0,
        }
    }

    /// Absorb message bytes. Any chunking produces the same tag.
    pub fn update(&mut self, mut data: &[u8]) {
        if self.buf_len > 0 {
            let take = core::cmp::min(16 - self.buf_len, data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len == 16 {
                let block = self.buf;
                self.process_block(&block, true);
                self.buf_len = 0;
            }
        }
        while data.len() >= 16 {
            let (blk, rest) = data.split_at(16);
            let mut block = [0u8; 16];
            block.copy_from_slice(blk);
            self.process_block(&block, true);
            data = rest;
        }
        if !data.is_empty() {
            // Zero first: any bytes beyond the new leftover must read as
            // zero when the partial block is finalized (stale bytes from a
            // previously buffered block would otherwise leak into the MAC).
            self.buf = [0u8; 16];
            self.buf[..data.len()].copy_from_slice(data);
            self.buf_len = data.len();
        }
    }

    /// Pad the streamed length to a 16-byte boundary (RFC 8439 §2.8 AEAD
    /// construction). No-op when already aligned or nothing absorbed.
    pub fn pad16(&mut self) {
        let rem = self.buf_len & 15;
        if rem != 0 {
            let zeros = [0u8; 16];
            self.update(&zeros[..16 - rem]);
        }
    }

    /// Absorb a little-endian u64 length word (AEAD trailer).
    pub fn update_le64(&mut self, value: u64) {
        self.update(&value.to_le_bytes());
    }

    /// h += block as a 129-bit little-endian integer (top bit set), then
    /// h *= r mod 2^130 - 5.
    fn process_block(&mut self, block: &[u8; 16], full: bool) {
        let le32 = |b: &[u8]| u32::from_le_bytes(b.try_into().unwrap());
        let t0 = le32(&block[0..4]);
        let t1 = le32(&block[4..8]);
        let t2 = le32(&block[8..12]);
        let t3 = le32(&block[12..16]);
        let hibit = if full { 1 << 24 } else { 0 };
        self.h[0] += t0 & MASK26;
        self.h[1] += ((t0 >> 26) | (t1 << 6)) & MASK26;
        self.h[2] += ((t1 >> 20) | (t2 << 12)) & MASK26;
        self.h[3] += ((t2 >> 14) | (t3 << 18)) & MASK26;
        self.h[4] += (t3 >> 8) | hibit;
        self.multiply();
    }

    /// h = (h * r) mod 2^130 - 5 with full carry propagation.
    fn multiply(&mut self) {
        let h = self.h;
        let r = self.r;
        let s1 = (r[1]) * 5;
        let s2 = (r[2]) * 5;
        let s3 = (r[3]) * 5;
        let s4 = (r[4]) * 5;
        let mut d = [
            (h[0] as u64) * (r[0] as u64)
                + (h[1] as u64) * (s4 as u64)
                + (h[2] as u64) * (s3 as u64)
                + (h[3] as u64) * (s2 as u64)
                + (h[4] as u64) * (s1 as u64),
            (h[0] as u64) * (r[1] as u64)
                + (h[1] as u64) * (r[0] as u64)
                + (h[2] as u64) * (s4 as u64)
                + (h[3] as u64) * (s3 as u64)
                + (h[4] as u64) * (s2 as u64),
            (h[0] as u64) * (r[2] as u64)
                + (h[1] as u64) * (r[1] as u64)
                + (h[2] as u64) * (r[0] as u64)
                + (h[3] as u64) * (s4 as u64)
                + (h[4] as u64) * (s3 as u64),
            (h[0] as u64) * (r[3] as u64)
                + (h[1] as u64) * (r[2] as u64)
                + (h[2] as u64) * (r[1] as u64)
                + (h[3] as u64) * (r[0] as u64)
                + (h[4] as u64) * (s4 as u64),
            (h[0] as u64) * (r[4] as u64)
                + (h[1] as u64) * (r[3] as u64)
                + (h[2] as u64) * (r[2] as u64)
                + (h[3] as u64) * (r[1] as u64)
                + (h[4] as u64) * (r[0] as u64),
        ];
        let mut c = (d[0] >> 26) as u32;
        d[0] &= MASK26 as u64;
        d[1] += c as u64;
        c = (d[1] >> 26) as u32;
        d[1] &= MASK26 as u64;
        d[2] += c as u64;
        c = (d[2] >> 26) as u32;
        d[2] &= MASK26 as u64;
        d[3] += c as u64;
        c = (d[3] >> 26) as u32;
        d[3] &= MASK26 as u64;
        d[4] += c as u64;
        c = (d[4] >> 26) as u32;
        d[4] &= MASK26 as u64;
        // Fold the 2^130 term back in: 2^130 ≡ 5 (mod p). The residual on
        // h1 is intentionally left un-masked (≤ 2^26 + 1); the next
        // multiply's u64 products and finalize's carry chain absorb it.
        let h0 = (d[0] as u32) + c * 5;
        let c = h0 >> 26;
        self.h[0] = h0 & MASK26;
        self.h[1] = (d[1] as u32) + c;
        self.h[2] = d[2] as u32;
        self.h[3] = d[3] as u32;
        self.h[4] = d[4] as u32;
    }

    /// Finalize: process any buffered partial block (the RFC appends a
    /// 0x01 byte before zero-padding it to 16), fully carry h, reduce mod
    /// p, add s, return the 16-byte tag (RFC 8439 §2.5).
    pub fn finalize(mut self) -> [u8; 16] {
        if self.buf_len > 0 {
            let mut block = self.buf;
            block[self.buf_len] = 1;
            self.buf_len = 0;
            self.process_block(&block, false);
        }
        let mut h = self.h;
        // Carry so every limb is canonical before the p comparison.
        let mut carry = h[1] >> 26;
        h[1] &= MASK26;
        h[2] += carry;
        carry = h[2] >> 26;
        h[2] &= MASK26;
        h[3] += carry;
        carry = h[3] >> 26;
        h[3] &= MASK26;
        h[4] += carry;
        carry = h[4] >> 26;
        h[4] &= MASK26;
        h[0] += carry * 5;
        carry = h[0] >> 26;
        h[0] &= MASK26;
        h[1] += carry;
        // h + 5 - p: if that addition carries out of 2^130, h >= p and the
        // reduced value is the sum; otherwise keep h. Masked, not branched.
        let mut g = h;
        g[0] = g[0].wrapping_add(5);
        let mut c = g[0] >> 26;
        g[0] &= MASK26;
        g[1] = g[1].wrapping_add(c);
        c = g[1] >> 26;
        g[1] &= MASK26;
        g[2] = g[2].wrapping_add(c);
        c = g[2] >> 26;
        g[2] &= MASK26;
        g[3] = g[3].wrapping_add(c);
        c = g[3] >> 26;
        g[3] &= MASK26;
        g[4] = g[4].wrapping_add(c).wrapping_sub(1 << 26);
        // g[4] bit 31 set means h + 5 stayed below 2^130, i.e. h < p: keep
        // h (mask 0); otherwise h >= p and the reduced g applies (all ones).
        let mask = (g[4] >> 31).wrapping_sub(1);
        for i in 0..5 {
            h[i] = (h[i] & !mask) | (g[i] & mask);
        }
        // Repack the 26-bit limbs into the 128-bit little-endian integer
        // h mod 2^130, then tag = (h + s) mod 2^128.
        let h0 = (h[0] | (h[1] << 26)) & 0xffff_ffff;
        let h1 = ((h[1] >> 6) | (h[2] << 20)) & 0xffff_ffff;
        let h2 = ((h[2] >> 12) | (h[3] << 14)) & 0xffff_ffff;
        let h3 = ((h[3] >> 18) | (h[4] << 8)) & 0xffff_ffff;
        let f0 = (h0 as u64) + (self.pad[0] as u64);
        let f1 = (h1 as u64) + (self.pad[1] as u64) + (f0 >> 32);
        let f2 = (h2 as u64) + (self.pad[2] as u64) + (f1 >> 32);
        let f3 = (h3 as u64) + (self.pad[3] as u64) + (f2 >> 32);
        let words = [f0 as u32, f1 as u32, f2 as u32, f3 as u32];
        let mut tag = [0u8; 16];
        tag[0..4].copy_from_slice(&words[0].to_le_bytes());
        tag[4..8].copy_from_slice(&words[1].to_le_bytes());
        tag[8..12].copy_from_slice(&words[2].to_le_bytes());
        tag[12..16].copy_from_slice(&words[3].to_le_bytes());
        tag
    }
}

/// One-shot Poly1305 tag over `message`.
pub fn poly1305(message: &[u8], key: &[u8; 32]) -> [u8; 16] {
    let mut mac = Poly1305::new(key);
    mac.update(message);
    mac.finalize()
}

#[cfg(test)]
mod tests_poly1305 {
    use super::*;

    fn unhex<const N: usize>(s: &str) -> [u8; N] {
        let mut o = [0u8; N];
        for i in 0..N {
            o[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap();
        }
        o
    }

    /// RFC 8439 §2.5.2 example: 34-byte message, key 85:d6:be:78:...
    #[test]
    fn rfc8439_2_5_2_example() {
        let key = unhex::<32>("85d6be7857556d337f4452fe42d506a80103808afb0db2fd4abff6af4149f51b");
        let tag = poly1305(b"Cryptographic Forum Research Group", &key);
        assert_eq!(tag, unhex::<16>("a8061dc1305136c6c22b8baf0c0127a9"));
    }

    /// RFC 8439 §A.3 test vector #1: all-zero key and 64-byte zero message
    /// produce an all-zero tag.
    #[test]
    fn rfc8439_a3_vector1() {
        let tag = poly1305(&[0u8; 64], &[0u8; 32]);
        assert_eq!(tag, [0u8; 16]);
    }

    /// RFC 8439 §A.3 test vector #2: r = 0, so the tag equals s regardless
    /// of the 375-byte message.
    #[test]
    fn rfc8439_a3_vector2() {
        let mut key = [0u8; 32];
        key[16..].copy_from_slice(&unhex::<16>("36e5f6b5c5e06070f0efca96227a863e"));
        let msg = b"Any submission to the IETF intended by the Contributor for publication \
as all or part of an IETF Internet-Draft or RFC and any statement made within the \
context of an IETF activity is considered an \"IETF Contribution\". Such statements \
include oral statements in IETF sessions, as well as written and electronic \
communications made at any time or place, which are addressed to";
        let tag = poly1305(msg, &key);
        assert_eq!(tag, unhex::<16>("36e5f6b5c5e06070f0efca96227a863e"));
    }

    /// RFC 8439 §A.3 test vector #4: 127-byte "'Twas brillig" text.
    #[test]
    fn rfc8439_a3_vector4() {
        let key = unhex::<32>("1c9240a5eb55d38af333888604f6b5f0473917c1402b80099dca5cbc207075c0");
        let msg = b"'Twas brillig, and the slithy toves\nDid gyre and gimble in the wabe:\n\
All mimsy were the borogoves,\nAnd the mome raths outgrabe.";
        let tag = poly1305(msg, &key);
        assert_eq!(
            tag,
            [
                0x45u8, 0x41, 0x66, 0x9a, 0x7e, 0xaa, 0xee, 0x61, 0xe7, 0x08, 0xdc, 0x7c, 0xbc,
                0xc5, 0xeb, 0x62
            ]
        );
    }

    /// RFC 8439 §A.3 test vector #5: 130-bit partial reduction edge — a
    /// full 0xff block with r = 2 must fully reduce to 3.
    #[test]
    fn rfc8439_a3_vector5() {
        let mut key = [0u8; 32];
        key[0] = 2;
        let tag = poly1305(&[0xffu8; 16], &key);
        let mut expect = [0u8; 16];
        expect[0] = 3;
        assert_eq!(tag, expect);
    }

    /// RFC 8439 §A.3 test vector #7: data limb all ones with carry from a
    /// lower limb.
    #[test]
    fn rfc8439_a3_vector7() {
        let mut key = [0u8; 32];
        key[0] = 1;
        let mut msg = [0xffu8; 48];
        msg[16] = 0xf0;
        msg[32] = 0x11;
        msg[33..].fill(0);
        let tag = poly1305(&msg, &key);
        let mut expect = [0u8; 16];
        expect[0] = 5;
        assert_eq!(tag, expect);
    }

    /// RFC 8439 §A.3 test vector #8: polynomial result exactly 2^130 - 5.
    #[test]
    fn rfc8439_a3_vector8() {
        let mut key = [0u8; 32];
        key[0] = 1;
        let mut msg = [0u8; 48];
        msg[..16].fill(0xff);
        msg[16] = 0xfb;
        msg[17..32].fill(0xfe);
        msg[32..].fill(0x01);
        assert_eq!(poly1305(&msg, &key), [0u8; 16]);
    }

    /// RFC 8439 §A.3 test vector #9: polynomial result exactly 2^130 - 6.
    #[test]
    fn rfc8439_a3_vector9() {
        let mut key = [0u8; 32];
        key[0] = 2;
        let mut msg = [0xffu8; 16];
        msg[0] = 0xfd;
        let tag = poly1305(&msg, &key);
        let mut expect = [0xffu8; 16];
        expect[0] = 0xfa;
        assert_eq!(tag, expect);
    }

    /// RFC 8439 §A.3 test vector #11: 5*H+L-type reduction producing a
    /// 131-bit final result.
    #[test]
    fn rfc8439_a3_vector11() {
        let mut key = [0u8; 32];
        key[0] = 1;
        key[8] = 4;
        let msg = unhex::<48>(
            "e33594d7505e43b90000000000000000\
             3394d7505e4379cd0100000000000000\
             00000000000000000000000000000000",
        );
        let tag = poly1305(&msg, &key);
        let mut expect = [0u8; 16];
        expect[0] = 0x13;
        assert_eq!(tag, expect);
    }

    /// Incremental feeding (1-byte chunks) must equal the one-shot tag.
    #[test]
    fn incremental_matches_one_shot() {
        let key = unhex::<32>("85d6be7857556d337f4452fe42d506a80103808afb0db2fd4abff6af4149f51b");
        let msg = b"streaming must produce identical tags regardless of chunking";
        let one_shot = poly1305(msg, &key);
        let mut mac = Poly1305::new(&key);
        for byte in msg.iter() {
            mac.update(core::slice::from_ref(byte));
        }
        assert_eq!(mac.finalize(), one_shot);
    }
}
