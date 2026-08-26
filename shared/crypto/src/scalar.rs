//! Scalars modulo the Ed25519 group order
//! L = 2^252 + 27742317777372353535851937790883648493.

#[derive(Clone, Copy, Debug)]
pub struct Scalar(pub [u8; 32]);

const L_LIMBS: [u64; 4] = [
    0x5812_631a_5cf5_d3ed,
    0x14de_f9de_a2f7_9cd6,
    0x0000_0000_0000_0000,
    0x1000_0000_0000_0000,
];

pub fn le_u64x4(bytes: &[u8; 32]) -> [u64; 4] {
    [
        u64::from_le_bytes(bytes[0..8].try_into().unwrap()),
        u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
        u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
        u64::from_le_bytes(bytes[24..32].try_into().unwrap()),
    ]
}

pub fn le_bytes(words: &[u64; 4]) -> [u8; 32] {
    let mut out = [0u8; 32];
    for i in 0..4 {
        out[i * 8..(i + 1) * 8].copy_from_slice(&words[i].to_le_bytes());
    }
    out
}

fn shl1(acc: &mut [u64; 4], bit: u64) {
    let mut carry = bit & 1;
    for limb in acc.iter_mut() {
        let next = *limb >> 63;
        *limb = (*limb << 1) | carry;
        carry = next;
    }
}

/// Branchless conditional subtract of L when acc >= L.
/// Borrow chain must be recomputed per limb (L has sparse limbs, so a
/// sticky OR-ed borrow would misclassify legitimate absorptions).
fn csub_l(acc: &mut [u64; 4]) {
    let mut borrow = 0u64;
    let mut d = [0u64; 4];
    for i in 0..4 {
        let (rhs, ovf) = L_LIMBS[i].overflowing_add(borrow);
        let (v, br) = acc[i].overflowing_sub(rhs);
        d[i] = v;
        borrow = (br | ovf) as u64;
    }
    let take = (borrow == 0) as u64;
    let mask = take.wrapping_neg();
    for i in 0..4 {
        acc[i] ^= mask & (acc[i] ^ d[i]);
    }
}

impl Scalar {
    /// Reduce a 512-bit little-endian integer modulo L. Constant-time:
    /// fixed 512 iterations with masked conditional subtractions.
    pub fn reduce_wide(bytes: &[u8; 64]) -> Scalar {
        let mut acc = [0u64; 4];
        for bit in (0..512).rev() {
            let b = ((bytes[bit >> 3] >> (bit & 7)) & 1) as u64;
            shl1(&mut acc, b);
            csub_l(&mut acc);
        }
        Scalar(le_bytes(&acc))
    }

    /// Canonical scalar check: value < L.
    pub fn is_canonical(bytes: &[u8; 32]) -> bool {
        let s = le_u64x4(bytes);
        // Compare big-endian by limb significance (index 3 is most significant).
        for i in (0..4).rev() {
            if s[i] != L_LIMBS[i] {
                return s[i] < L_LIMBS[i];
            }
        }
        false // equal to L is not canonical
    }

    /// (a * b + c) mod L.
    pub fn mul_add(a: &Scalar, b: &Scalar, c: &Scalar) -> Scalar {
        let aw = le_u64x4(&a.0);
        let bw = le_u64x4(&b.0);
        let cw = le_u64x4(&c.0);
        let mut wide = [0u64; 8];
        for i in 0..4 {
            let mut carry: u128 = 0;
            for j in 0..4 {
                let t = wide[i + j] as u128 + (aw[i] as u128) * (bw[j] as u128) + carry;
                wide[i + j] = t as u64;
                carry = t >> 64;
            }
            wide[i + 4] = (wide[i + 4] as u128 + carry) as u64;
        }
        // Add c into the low half, propagating any carry upward.
        let mut carry: u128 = 0;
        for i in 0..4 {
            let t = wide[i] as u128 + cw[i] as u128 + carry;
            wide[i] = t as u64;
            carry = t >> 64;
        }
        let mut idx = 4;
        while carry > 0 && idx < 8 {
            let t = wide[idx] as u128 + carry;
            wide[idx] = t as u64;
            carry = t >> 64;
            idx += 1;
        }
        let mut bytes = [0u8; 64];
        for i in 0..8 {
            bytes[i * 8..(i + 1) * 8].copy_from_slice(&wide[i].to_le_bytes());
        }
        Scalar::reduce_wide(&bytes)
    }

}

#[cfg(test)]
impl Scalar {
    pub(crate) fn add_mod_for_test(&self, other: &Scalar) -> Scalar {
        let aw = le_u64x4(&self.0);
        let bw = le_u64x4(&other.0);
        let mut wide = [0u64; 8];
        for i in 0..4 {
            let t = aw[i] as u128 + bw[i] as u128;
            wide[i] = t as u64;
            wide[i + 1] = (t >> 64) as u64;
        }
        let mut bytes = [0u8; 64];
        for i in 0..8 {
            bytes[i * 8..(i + 1) * 8].copy_from_slice(&wide[i].to_le_bytes());
        }
        Scalar::reduce_wide(&bytes)
    }
}

#[cfg(test)]
mod tests_scalar {
    use super::*;

    #[test]
    fn reduce_wide_l_is_zero() {
        let mut wide = [0u8; 64];
        wide[..32].copy_from_slice(&le_bytes(&L_LIMBS));
        assert_eq!(Scalar::reduce_wide(&wide).0, [0u8; 32]);
    }

    #[test]
    fn mul_add_commutes_and_matches_tiny() {
        let one = Scalar({ let mut x = [0u8; 32]; x[0] = 1; x });
        let mut ka = [0u8; 32];
        ka[0] = 0x37;
        ka[10] = 0xab;
        ka[20] = 0xcd;
        let mut kb = [0u8; 32];
        kb[5] = 0x91;
        kb[25] = 0x77;
        let ab = Scalar::mul_add(&Scalar(ka), &Scalar(kb), &one);
        let ba = Scalar::mul_add(&Scalar(kb), &Scalar(ka), &one);
        assert_eq!(ab.0, ba.0, "k*a+r commutes");
        let big = Scalar::reduce_wide(&{
            let mut w = [0u8; 64];
            w[0] = 2;
            w
        });
        assert_eq!(Scalar::mul_add(&one, &one, &one).0, big.0, "1*1+1 == 2");
    }

    #[test]
    fn canonical_boundaries() {
        // L - 1 is canonical, L and anything above are not.
        let mut lm1 = le_bytes(&L_LIMBS);
        lm1[0] -= 1;
        assert!(Scalar::is_canonical(&lm1));
        let l = le_bytes(&L_LIMBS);
        assert!(!Scalar::is_canonical(&l));
        let mut gt = l;
        gt[0] += 1;
        assert!(!Scalar::is_canonical(&gt));
    }

    fn wide64(low: &[u8]) -> [u8; 64] {
        let mut w = [0u8; 64];
        w[..low.len()].copy_from_slice(low);
        w
    }

    fn unhex32(s: &str) -> [u8; 32] {
        let mut o = [0u8; 32];
        let b = s.as_bytes();
        for i in 0..32 {
            let hi = (b[i * 2] as char).to_digit(16).unwrap() as u8;
            let lo = (b[i * 2 + 1] as char).to_digit(16).unwrap() as u8;
            o[i] = (hi << 4) | lo;
        }
        o
    }

    #[test]
    fn reduce_wide_kats() {
        let l_bytes = le_bytes(&L_LIMBS);
        let mut wide3 = [0u8; 64];
        let mut carry = 0u16;
        for i in 0..32 {
            let t = 3 * l_bytes[i] as u16 + carry;
            wide3[i] = t as u8;
            carry = t >> 8;
        }
        wide3[32] = carry as u8;
        let mut l_plus_5_input = wide64(&l_bytes);
        l_plus_5_input[0] += 5;
        let mut pow255 = [0u8; 64];
        pow255[31] = 0x80;
        let all_ff = [0xffu8; 64];
        let cases: &[(&str, [u8; 64], &str)] = &[
            ("five", wide64(&[5]), "0500000000000000000000000000000000000000000000000000000000000000"),
            ("l", wide64(&l_bytes), "0000000000000000000000000000000000000000000000000000000000000000"),
            ("l_plus_5", l_plus_5_input, "0500000000000000000000000000000000000000000000000000000000000000"),
            ("three_l", wide3, "0000000000000000000000000000000000000000000000000000000000000000"),
            ("two_pow_255", pow255, "85344775474a7f9723b63a8be92ae76dffffffffffffffffffffffffffffff0f"),
            ("all_ff", all_ff, "000f9c44e31106a447938568a71b0ed065bef517d273ecce3d9a307c1b419903"),
        ];
        for (name, input, expect_hex) in cases.iter() {
            let got = Scalar::reduce_wide(input);
            assert_eq!(got.0, unhex32(expect_hex), "reduce {}", name);
        }
    }
}
