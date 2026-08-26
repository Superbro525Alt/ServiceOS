//! Twisted Edwards curve points (Ed25519) in extended coordinates.
//!
//! Representation: (X : Y : Z : T) with x = X/Z, y = Y/Z, T = XY/Z.
//! Formulas follow RFC 8032 section 5.1 (unified addition, a = -1).

use crate::field::Fe;
use crate::scalar::Scalar;

pub struct Point {
    pub x: Fe,
    pub y: Fe,
    pub z: Fe,
    pub t: Fe,
}

/// d = -121665/121666 mod p.
const D_BYTES: [u8; 32] = [
    0xa3, 0x78, 0x59, 0x13, 0xca, 0x4d, 0xeb, 0x75, 0xab, 0xd8, 0x41, 0x41, 0x4d, 0x0a, 0x70, 0x00,
    0x98, 0xe8, 0x79, 0x77, 0x79, 0x40, 0xc7, 0x8c, 0x73, 0xfe, 0x6f, 0x2b, 0xee, 0x6c, 0x03, 0x52,
];

/// Compressed encoding of the Ed25519 basepoint (y = 4/5, x even).
const BASE_COMPRESSED: [u8; 32] = [
    0x58, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
    0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
];

impl Point {
    pub fn identity() -> Point {
        Point {
            x: Fe::ZERO,
            y: Fe::ONE,
            z: Fe::ONE,
            t: Fe::ZERO,
        }
    }

    pub fn base() -> Point {
        // The canonical basepoint always decompresses.
        match decompress(&BASE_COMPRESSED) {
            Some(p) => p,
            None => Point::identity(),
        }
    }

    /// Unified addition (RFC 8032 5.1.4), valid for all pairs on this curve.
    pub fn add(&self, q: &Point) -> Point {
        let d2 = Fe::from_bytes(&D_BYTES).add(&Fe::from_bytes(&D_BYTES));
        let a = self.y.sub(&self.x).mul(&q.y.sub(&q.x));
        let b = self.y.add(&self.x).mul(&q.y.add(&q.x));
        let c = self.t.mul(&d2).mul(&q.t);
        let d = self.z.mul(&q.z);
        let dd = d.add(&d);
        let e = b.sub(&a);
        let f = dd.sub(&c);
        let g = dd.add(&c);
        let h = b.add(&a);
        Point {
            x: e.mul(&f),
            y: g.mul(&h),
            t: e.mul(&h),
            z: f.mul(&g),
        }
    }

    /// Doubling (RFC 8032 5.1.4 doubling formulas).
    pub fn double(&self) -> Point {
        let a = self.x.square();
        let b = self.y.square();
        let c = self.z.square();
        let cc = c.add(&c);
        let h = a.add(&b);
        let xy = self.x.add(&self.y);
        let e = h.sub(&xy.square());
        let g = a.sub(&b);
        let f = cc.add(&g);
        Point {
            x: e.mul(&f),
            y: g.mul(&h),
            t: e.mul(&h),
            z: f.mul(&g),
        }
    }

    /// Branchless point select: choice != 0 picks `a`, else `b`.
    pub fn select(a: &Point, b: &Point, choice: u64) -> Point {
        Point {
            x: Fe::select(&a.x, &b.x, choice),
            y: Fe::select(&a.y, &b.y, choice),
            z: Fe::select(&a.z, &b.z, choice),
            t: Fe::select(&a.t, &b.t, choice),
        }
    }

    /// Constant-time-ish scalar multiplication: every bit is processed and
    /// the accumulate step uses masked selection (no secret-dependent
    /// branches or memory access).
    pub fn mul_scalar(&self, s: &Scalar) -> Point {
        let bytes = s.0;
        let mut r = Point::identity();
        for bit in (0..255).rev() {
            let b = ((bytes[bit >> 3] >> (bit & 7)) & 1) as u64;
            r = r.double();
            let sum = r.add(self);
            r = Point::select(&sum, &r, b);
        }
        r
    }

    pub fn eq(a: &Point, b: &Point) -> bool {
        // Operates on public data only (verification inputs).
        Fe::ct_eq(&a.x.mul(&b.z), &b.x.mul(&a.z))
            && Fe::ct_eq(&a.y.mul(&b.z), &b.y.mul(&a.z))
    }

    /// Compress to the 32-byte RFC 8032 encoding.
    pub fn compress(&self) -> [u8; 32] {
        let zinv = self.z.invert();
        let x = self.x.mul(&zinv);
        let mut out = self.y.mul(&zinv).to_bytes();
        if x.is_negative() {
            out[31] |= 0x80;
        }
        out
    }
}

/// Recover a point from its compressed encoding (RFC 8032 5.1.3).
/// Returns None for non-canonical y or when x has no square root.
pub fn decompress(bytes: &[u8; 32]) -> Option<Point> {
    let sign = bytes[31] >> 7 == 1;
    let mut y_bytes = *bytes;
    y_bytes[31] &= 0x7f;
    let y = Fe::from_bytes(&y_bytes);
    // Canonical check: re-encoding must round-trip.
    if y.to_bytes() != y_bytes {
        return None;
    }
    let yy = y.square();
    let u = yy.sub(&Fe::ONE);
    let v = yy.mul(&Fe::from_bytes(&D_BYTES)).add(&Fe::ONE);
    if v.is_zero() {
        // x^2 = u/v has no solution on the curve.
        return None;
    }
    let w = u.mul(&v.invert());
    // Candidate root of w: w^((p+3)/8) since p = 5 (mod 8).
    let (t19, _) = w.pow22501();
    let mut root = t19.pow2k(2).mul(&w.square());
    if !Fe::ct_eq(&root.square(), &w) {
        root = root.mul(&Fe::sqrt_m1());
        if !Fe::ct_eq(&root.square(), &w) {
            return None;
        }
    }
    // Match the requested parity.
    if root.is_negative() != sign {
        root = root.negate();
    }
    Some(Point {
        t: root.mul(&y),
        x: root,
        y,
        z: Fe::ONE,
    })
}

#[cfg(test)]
mod dbg5 {
    use super::*;
    use crate::scalar::Scalar;

    fn unhex(s: &str) -> [u8; 32] {
        let mut o = [0u8; 32];
        for i in 0..32 {
            let hi = (s.as_bytes()[i * 2] as char).to_digit(16).unwrap() as u8;
            let lo = (s.as_bytes()[i * 2 + 1] as char).to_digit(16).unwrap() as u8;
            o[i] = (hi << 4) | lo;
        }
        o
    }

    #[test]
    fn small_scalars_fixed() {
        let b = Point::base();
        assert_eq!(b.mul_scalar(&Scalar({ let mut x=[0u8;32]; x[0]=1; x })).compress(), unhex("5866666666666666666666666666666666666666666666666666666666666666"), "[1]B");
        assert_eq!(b.mul_scalar(&Scalar({ let mut x=[0u8;32]; x[0]=2; x })).compress(), unhex("c9a3f86aae465f0e56513864510f3997561fa2c9e85ea21dc2292309f3cd6022"), "[2]B");
        assert_eq!(b.mul_scalar(&Scalar({ let mut x=[0u8;32]; x[0]=3; x })).compress(), unhex("d4b4f5784868c3020403246717ec169ff79e26608ea126a1ab69ee77d1b16712"), "[3]B");
        assert_eq!(b.add(&b).compress(), unhex("c9a3f86aae465f0e56513864510f3997561fa2c9e85ea21dc2292309f3cd6022"), "add(B,B)");
        assert_eq!(b.double().compress(), unhex("c9a3f86aae465f0e56513864510f3997561fa2c9e85ea21dc2292309f3cd6022"), "dbl(B)");
    }

    fn msb_ladder(p: Point, bytes: &[u8; 32]) -> Point {
        let mut acc = Point::identity();
        let mut addend = p;
        for i in 0..255 {
            if ((bytes[i >> 3] >> (i & 7)) & 1) == 1 {
                acc = acc.add(&addend);
            }
            addend = addend.double();
        }
        acc
    }

    #[test]
    fn ladders_agree_on_large_scalars() {
        let b = Point::base();
        let mut clamped = unhex("5866666666666666666666666666666666666666666666666666666666666666");
        clamped[0] &= 248;
        clamped[31] = 0x40;
        let s = Scalar(clamped);
        assert_eq!(
            b.mul_scalar(&s).compress(),
            msb_ladder(Point::base(), &clamped).compress(),
            "msb-first vs lsb-first ladder"
        );
        let mut mixed = unhex("deadbeefcafebabe0123456789abcdef00112233445566778899aabbccddeeff");
        mixed[31] |= 0x40;
        let s2 = Scalar(mixed);
        assert_eq!(
            b.mul_scalar(&s2).compress(),
            msb_ladder(Point::base(), &mixed).compress(),
            "mixed-bit ladder agreement"
        );
    }

    #[test]
    fn distributes_over_large_scalars() {
        let b = Point::base();
        let sa = Scalar({
            let mut x = [0u8; 32];
            x[0] = 0x21;
            x[1] = 0xa3;
            x[15] = 0xff;
            x[31] = 0x40;
            x
        });
        let sb = Scalar({
            let mut x = [0u8; 32];
            x[2] = 0x7f;
            x[30] = 0x11;
            x
        });
        let pab = b.mul_scalar(&sa.add_mod_for_test(&sb)).compress();
        let pa_pb = b.mul_scalar(&sa).add(&b.mul_scalar(&sb)).compress();
        assert_eq!(pab, pa_pb, "(sa+sb)B == saB + sbB");
    }
}
