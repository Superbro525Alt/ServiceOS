//! SHA-512 (FIPS 180-4), pure-Rust, no_std.

const K: [u64; 80] = [
    0x428a2f98d728ae22,
    0x7137449123ef65cd,
    0xb5c0fbcfec4d3b2f,
    0xe9b5dba58189dbbc,
    0x3956c25bf348b538,
    0x59f111f1b605d019,
    0x923f82a4af194f9b,
    0xab1c5ed5da6d8118,
    0xd807aa98a3030242,
    0x12835b0145706fbe,
    0x243185be4ee4b28c,
    0x550c7dc3d5ffb4e2,
    0x72be5d74f27b896f,
    0x80deb1fe3b1696b1,
    0x9bdc06a725c71235,
    0xc19bf174cf692694,
    0xe49b69c19ef14ad2,
    0xefbe4786384f25e3,
    0x0fc19dc68b8cd5b5,
    0x240ca1cc77ac9c65,
    0x2de92c6f592b0275,
    0x4a7484aa6ea6e483,
    0x5cb0a9dcbd41fbd4,
    0x76f988da831153b5,
    0x983e5152ee66dfab,
    0xa831c66d2db43210,
    0xb00327c898fb213f,
    0xbf597fc7beef0ee4,
    0xc6e00bf33da88fc2,
    0xd5a79147930aa725,
    0x06ca6351e003826f,
    0x142929670a0e6e70,
    0x27b70a8546d22ffc,
    0x2e1b21385c26c926,
    0x4d2c6dfc5ac42aed,
    0x53380d139d95b3df,
    0x650a73548baf63de,
    0x766a0abb3c77b2a8,
    0x81c2c92e47edaee6,
    0x92722c851482353b,
    0xa2bfe8a14cf10364,
    0xa81a664bbc423001,
    0xc24b8b70d0f89791,
    0xc76c51a30654be30,
    0xd192e819d6ef5218,
    0xd69906245565a910,
    0xf40e35855771202a,
    0x106aa07032bbd1b8,
    0x19a4c116b8d2d0c8,
    0x1e376c085141ab53,
    0x2748774cdf8eeb99,
    0x34b0bcb5e19b48a8,
    0x391c0cb3c5c95a63,
    0x4ed8aa4ae3418acb,
    0x5b9cca4f7763e373,
    0x682e6ff3d6b2b8a3,
    0x748f82ee5defb2fc,
    0x78a5636f43172f60,
    0x84c87814a1f0ab72,
    0x8cc702081a6439ec,
    0x90befffa23631e28,
    0xa4506cebde82bde9,
    0xbef9a3f7b2c67915,
    0xc67178f2e372532b,
    0xca273eceea26619c,
    0xd186b8c721c0c207,
    0xeada7dd6cde0eb1e,
    0xf57d4f7fee6ed178,
    0x06f067aa72176fba,
    0x0a637dc5a2c898a6,
    0x113f9804bef90dae,
    0x1b710b35131c471b,
    0x28db77f523047d84,
    0x32caab7b40c72493,
    0x3c9ebe0a15c9bebc,
    0x431d67c49c100d4c,
    0x4cc5d4becb3e42b6,
    0x597f299cfc657e2a,
    0x5fcb6fab3ad6faec,
    0x6c44198c4a475817,
];

const H_INIT: [u64; 8] = [
    0x6a09e667f3bcc908,
    0xbb67ae8584caa73b,
    0x3c6ef372fe94f82b,
    0xa54ff53a5f1d36f1,
    0x510e527fade682d1,
    0x9b05688c2b3e6c1f,
    0x1f83d9abfb41bd6b,
    0x5be0cd19137e2179,
];

pub struct Sha512 {
    state: [u64; 8],
    buffer: [u8; 128],
    buffered: usize,
    length: u128,
}

impl Sha512 {
    pub fn new() -> Sha512 {
        Sha512 {
            state: H_INIT,
            buffer: [0; 128],
            buffered: 0,
            length: 0,
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u128);
        let mut rest = data;
        if self.buffered > 0 {
            let take = core::cmp::min(128 - self.buffered, rest.len());
            self.buffer[self.buffered..self.buffered + take].copy_from_slice(&rest[..take]);
            self.buffered += take;
            rest = &rest[take..];
            if self.buffered == 128 {
                let block = self.buffer;
                compress(&mut self.state, &block);
                self.buffered = 0;
            }
        }
        while rest.len() >= 128 {
            let mut block = [0u8; 128];
            block.copy_from_slice(&rest[..128]);
            compress(&mut self.state, &block);
            rest = &rest[128..];
        }
        if !rest.is_empty() {
            self.buffer[..rest.len()].copy_from_slice(rest);
            self.buffered = rest.len();
        }
    }

    pub fn finalize(mut self) -> [u8; 64] {
        let bits = self.length.wrapping_mul(8);
        // 0x80, zeros, 16-byte big-endian bit count.
        let mut chunk = [0u8; 128];
        chunk[..self.buffered].copy_from_slice(&self.buffer[..self.buffered]);
        chunk[self.buffered] = 0x80;
        if self.buffered < 112 {
            chunk[112..128].copy_from_slice(&bits.to_be_bytes());
            compress(&mut self.state, &chunk);
        } else {
            compress(&mut self.state, &chunk);
            let mut last = [0u8; 128];
            last[112..128].copy_from_slice(&bits.to_be_bytes());
            compress(&mut self.state, &last);
        }
        let mut out = [0u8; 64];
        for i in 0..8 {
            out[i * 8..(i + 1) * 8].copy_from_slice(&self.state[i].to_be_bytes());
        }
        out
    }
}

/// One-shot digest over concatenated parts.
pub fn digest(parts: &[&[u8]]) -> [u8; 64] {
    let mut h = Sha512::new();
    for part in parts {
        h.update(part);
    }
    h.finalize()
}

#[cfg(test)]
mod tests_sha {
    use super::*;

    #[test]
    fn kat_empty_prefix() {
        let d = digest(&[b""]);
        assert_eq!(d[0], 0xcf);
        assert_eq!(d[1], 0x83);
        assert_eq!(d[62], 0xda);
        assert_eq!(d[63], 0x3e);
    }

    #[test]
    fn kat_probe_19_bytes() {
        let data: [u8; 19] = [
            0x00, 0x01, 0x02, 0x03, 0x64, 0x65, 0x61, 0x64, 0x62, 0x65, 0x65, 0x66, 0x2d, 0x74,
            0x65, 0x73, 0x74, 0x2d, 0xff,
        ];
        let d = digest(&[&data]);
        assert_eq!((d[0], d[1]), (0x9e, 0x2e));
    }
}

fn compress(state: &mut [u64; 8], block: &[u8; 128]) {
    let mut w = [0u64; 80];
    for i in 0..16 {
        w[i] = u64::from_be_bytes(block[i * 8..(i + 1) * 8].try_into().unwrap());
    }
    for i in 16..80 {
        let s0 = w[i - 15].rotate_right(1) ^ w[i - 15].rotate_right(8) ^ (w[i - 15] >> 7);
        let s1 = w[i - 2].rotate_right(19) ^ w[i - 2].rotate_right(61) ^ (w[i - 2] >> 6);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }
    let mut v = *state;
    for i in 0..80 {
        let s1 = v[4].rotate_right(14) ^ v[4].rotate_right(18) ^ v[4].rotate_right(41);
        let ch = (v[4] & v[5]) ^ ((!v[4]) & v[6]);
        let t1 = v[7]
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(K[i])
            .wrapping_add(w[i]);
        let s0 = v[0].rotate_right(28) ^ v[0].rotate_right(34) ^ v[0].rotate_right(39);
        let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
        let t2 = s0.wrapping_add(maj);
        v[7] = v[6];
        v[6] = v[5];
        v[5] = v[4];
        v[4] = v[3].wrapping_add(t1);
        v[3] = v[2];
        v[2] = v[1];
        v[1] = v[0];
        v[0] = t1.wrapping_add(t2);
    }
    for i in 0..8 {
        state[i] = state[i].wrapping_add(v[i]);
    }
}
