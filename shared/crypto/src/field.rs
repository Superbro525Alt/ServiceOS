//! Arithmetic in GF(2^255 - 19) using five 51-bit limbs.
//!
//! Limb bounds are kept loose (< 2^54 on inputs to `mul`) so `add`/`sub`
//! never need intermediate reduction; `mul` fully reduces modulo p.

const MASK51: u64 = (1u64 << 51) - 1;

/// 2p in 51-bit limbs, used to keep subtraction non-negative.
/// Values: 2^52-38 and 2^52-2.
const TWO_P: [u64; 5] = [
    4503599627370458,
    4503599627370494,
    4503599627370494,
    4503599627370494,
    4503599627370494,
];

/// p in 51-bit limbs (for final canonicalization).
/// Values: 2^51-19 and 2^51-1.
const P_LIMBS: [u64; 5] = [
    2251799813685229,
    2251799813685247,
    2251799813685247,
    2251799813685247,
    2251799813685247,
];

#[derive(Clone, Copy, Debug)]
pub struct Fe(pub [u64; 5]);

fn m(x: u64, y: u64) -> u128 {
    (x as u128) * (y as u128)
}

impl Fe {
    pub const ZERO: Fe = Fe([0; 5]);
    pub const ONE: Fe = Fe([1, 0, 0, 0, 0]);

    pub fn from_u64(v: u64) -> Fe {
        Fe([v & MASK51, v >> 51, 0, 0, 0])
    }

    pub fn from_bytes(b: &[u8; 32]) -> Fe {
        Fe([
            u64::from_le_bytes(b[0..8].try_into().unwrap()) & MASK51,
            (u64::from_le_bytes(b[6..14].try_into().unwrap()) >> 3) & MASK51,
            (u64::from_le_bytes(b[12..20].try_into().unwrap()) >> 6) & MASK51,
            (u64::from_le_bytes(b[19..27].try_into().unwrap()) >> 1) & MASK51,
            (u64::from_le_bytes(b[24..32].try_into().unwrap()) >> 12) & MASK51,
        ])
    }

    /// Fully carry and canonically reduce; returns the unique little-endian
    /// encoding of the field element in [0, p).
    pub fn to_bytes(&self) -> [u8; 32] {
        let limbs = self.canonical_limbs();
        let q0 = limbs[0] | (limbs[1] << 51);
        let q1 = (limbs[1] >> 13) | (limbs[2] << 38);
        let q2 = (limbs[2] >> 26) | (limbs[3] << 25);
        let q3 = (limbs[3] >> 39) | (limbs[4] << 12);
        let mut out = [0u8; 32];
        out[0..8].copy_from_slice(&q0.to_le_bytes());
        out[8..16].copy_from_slice(&q1.to_le_bytes());
        out[16..24].copy_from_slice(&q2.to_le_bytes());
        out[24..32].copy_from_slice(&q3.to_le_bytes());
        out
    }

    fn canonical_limbs(&self) -> [u64; 5] {
        let mut l = self.carry_pass();
        // Conditional subtract of p (branchless on the borrow flag).
        let mut borrow = 0u64;
        let mut d = [0u64; 5];
        for i in 0..5 {
            let (v, b1) = l[i].overflowing_sub(P_LIMBS[i]);
            let (v2, b2) = v.overflowing_sub(borrow);
            d[i] = v2;
            borrow |= (b1 as u64) | (b2 as u64);
        }
        let mask = borrow.wrapping_sub(1); // all-ones iff l >= p
        for i in 0..5 {
            l[i] ^= mask & (l[i] ^ d[i]);
        }
        l
    }

    fn carry_pass(&self) -> [u64; 5] {
        let mut l = self.0;
        l[1] += l[0] >> 51;
        l[0] &= MASK51;
        l[2] += l[1] >> 51;
        l[1] &= MASK51;
        l[3] += l[2] >> 51;
        l[2] &= MASK51;
        l[4] += l[3] >> 51;
        l[3] &= MASK51;
        l[0] += (l[4] >> 51) * 19;
        l[4] &= MASK51;
        l[1] += l[0] >> 51;
        l[0] &= MASK51;
        l
    }

    pub fn add(&self, r: &Fe) -> Fe {
        let mut o = [0u64; 5];
        for i in 0..5 {
            o[i] = self.0[i] + r.0[i];
        }
        Fe(o)
    }

    pub fn sub(&self, r: &Fe) -> Fe {
        let mut o = [0u64; 5];
        for i in 0..5 {
            o[i] = self.0[i] + TWO_P[i] - r.0[i];
        }
        Fe(o)
    }

    pub fn negate(&self) -> Fe {
        Fe::ZERO.sub(self)
    }

    pub fn mul(&self, r: &Fe) -> Fe {
        let a = self.0;
        let b = r.0;
        let b1_19 = b[1] * 19;
        let b2_19 = b[2] * 19;
        let b3_19 = b[3] * 19;
        let b4_19 = b[4] * 19;
        let c0 = m(a[0], b[0]) + m(a[4], b1_19) + m(a[3], b2_19) + m(a[2], b3_19) + m(a[1], b4_19);
        let c1 = m(a[1], b[0]) + m(a[0], b[1]) + m(a[4], b2_19) + m(a[3], b3_19) + m(a[2], b4_19);
        let c2 = m(a[2], b[0]) + m(a[1], b[1]) + m(a[0], b[2]) + m(a[4], b3_19) + m(a[3], b4_19);
        let c3 = m(a[3], b[0]) + m(a[2], b[1]) + m(a[1], b[2]) + m(a[0], b[3]) + m(a[4], b4_19);
        let c4 = m(a[4], b[0]) + m(a[3], b[1]) + m(a[2], b[2]) + m(a[1], b[3]) + m(a[0], b[4]);
        from_wide([c0, c1, c2, c3, c4])
    }

    pub fn square(&self) -> Fe {
        self.mul(self)
    }

    /// Self^(2^k).
    pub fn pow2k(&self, k: u32) -> Fe {
        let mut r = *self;
        for _ in 0..k {
            r = r.square();
        }
        r
    }

    /// Returns (self^(2^250 - 1), self^11).
    pub(crate) fn pow22501(&self) -> (Fe, Fe) {
        let t0 = self.square(); // 2
        let t1 = t0.square().square(); // 8
        let t2 = self.mul(&t1); // 9
        let t3 = t0.mul(&t2); // 11
        let t4 = t3.square(); // 22
        let t5 = t2.mul(&t4); // 2^5 - 1
        let t6 = t5.pow2k(5);
        let t7 = t6.mul(&t5); // 2^10 - 1
        let t8 = t7.pow2k(10);
        let t9 = t8.mul(&t7); // 2^20 - 1
        let t10 = t9.pow2k(20);
        let t11 = t10.mul(&t9); // 2^40 - 1
        let t12 = t11.pow2k(10);
        let t13 = t12.mul(&t7); // 2^50 - 1
        let t14 = t13.pow2k(50);
        let t15 = t14.mul(&t13); // 2^100 - 1
        let t16 = t15.pow2k(100);
        let t17 = t16.mul(&t15); // 2^200 - 1
        let t18 = t17.pow2k(50);
        let t19 = t18.mul(&t13); // 2^250 - 1
        (t19, t3)
    }

    /// Multiplicative inverse via x^(p-2). Zero maps to zero.
    pub fn invert(&self) -> Fe {
        let (t19, t3) = self.pow22501();
        t19.pow2k(5).mul(&t3)
    }

    /// sqrt(-1) = 2^((p-1)/4); valid since p = 5 (mod 8).
    pub fn sqrt_m1() -> Fe {
        let two = Fe::from_u64(2);
        let two_cubed = two.square().mul(&two);
        let (t19, _) = two.pow22501();
        t19.pow2k(3).mul(&two_cubed)
    }

    /// Branchless select: choice != 0 picks `a`, else `b`.
    pub fn select(a: &Fe, b: &Fe, choice: u64) -> Fe {
        let mask = (choice & 1).wrapping_neg();
        let mut o = [0u64; 5];
        for i in 0..5 {
            o[i] = b.0[i] ^ (mask & (a.0[i] ^ b.0[i]));
        }
        Fe(o)
    }

    pub fn is_negative(&self) -> bool {
        self.to_bytes()[0] & 1 == 1
    }

    pub fn is_zero(&self) -> bool {
        self.to_bytes() == [0u8; 32]
    }

    pub fn ct_eq(a: &Fe, b: &Fe) -> bool {
        let (x, y) = (a.to_bytes(), b.to_bytes());
        constant_time_eq(&x, &y)
    }
}

fn from_wide(c: [u128; 5]) -> Fe {
    const M51_WIDE: u128 = MASK51 as u128;
    let mut l = [0u64; 5];
    let mut carry: u128 = 0;
    for i in 0..5 {
        let v = c[i] + carry;
        l[i] = (v & M51_WIDE) as u64;
        carry = v >> 51;
    }
    let fold = carry.wrapping_mul(19);
    l[0] += (fold & M51_WIDE) as u64;
    l[1] += (fold >> 51) as u64;
    l[1] += l[0] >> 51;
    l[0] &= MASK51;
    Fe(l)
}

pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    debug_assert_eq!(a.len(), b.len());
    let mut acc = 0u8;
    for i in 0..a.len() {
        acc |= a[i] ^ b[i];
    }
    acc == 0
}
