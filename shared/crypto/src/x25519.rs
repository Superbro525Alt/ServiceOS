//! X25519 Diffie-Hellman over Curve25519 (RFC 7748 §5/§6.1), pure `core`.
//!
//! Scalar multiplication uses the RFC 7748 Montgomery ladder over the
//! crate's GF(2^255-19) field (five 51-bit limbs, u128 products, no heap).
//! Every ladder step is executed regardless of the scalar bit; bit selection
//! happens only through the branchless `cswap`, so no secret-dependent
//! branches or memory access patterns exist.
//!
//! Per RFC 7748 §6: received u-coordinates have their top bit masked;
//! output is the bare u-coordinate (v2, non-canonical encodings of zero are
//! accepted on input). All-zero outputs (low-order peer points) are returned
//! rather than rejected, matching the RFC's permissive option — callers that
//! need strictness can check for the all-zero secret themselves.

use crate::field::Fe;

/// Base point u = 9 (RFC 7748 §6.1).
const BASEPOINT_U: [u8; 32] = {
    let mut u = [0u8; 32];
    u[0] = 9;
    u
};

const A24: u64 = 121665;

/// A Diffie-Hellman shared secret. Keep private; feed to a KDF (e.g.
/// `hkdf`) before use as key material.
pub struct SharedSecret([u8; 32]);

impl SharedSecret {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Clamp a scalar per RFC 7748 §5: clear the three low bits, clear the top
/// bit, set the second-highest bit.
fn clamp(scalar: &[u8; 32]) -> [u8; 32] {
    let mut k = *scalar;
    k[0] &= 248;
    k[31] &= 127;
    k[31] |= 64;
    k
}

/// Branchless conditional swap: `swap != 0` exchanges `a` and `b`.
fn cswap(swap: u64, a: &mut Fe, b: &mut Fe) {
    let mask = swap.wrapping_neg();
    for i in 0..5 {
        let t = mask & (a.0[i] ^ b.0[i]);
        a.0[i] ^= t;
        b.0[i] ^= t;
    }
}

/// Montgomery ladder: returns the u-coordinate of `k * P` where `P` is the
/// point with u-coordinate `u` (top bit already masked by the caller or
/// masked here for external inputs).
fn scalar_mult(k: &[u8; 32], u: &[u8; 32]) -> [u8; 32] {
    let mut u_masked = *u;
    u_masked[31] &= 127;
    let x1 = Fe::from_bytes(&u_masked);
    let mut x2 = Fe::ONE;
    let mut z2 = Fe::ZERO;
    let mut x3 = x1;
    let mut z3 = Fe::ONE;
    let mut swap = 0u64;
    for t in (0..255).rev() {
        let kt = ((k[t >> 3] >> (t & 7)) & 1) as u64;
        swap ^= kt;
        cswap(swap, &mut x2, &mut x3);
        cswap(swap, &mut z2, &mut z3);
        swap = kt;
        let a = x2.add(&z2);
        let aa = a.square();
        let b = x2.sub(&z2);
        let bb = b.square();
        let e = aa.sub(&bb);
        let c = x3.add(&z3);
        let d = x3.sub(&z3);
        let da = d.mul(&a);
        let cb = c.mul(&b);
        x3 = da.add(&cb).square();
        z3 = x1.mul(&da.sub(&cb).square());
        x2 = aa.mul(&bb);
        let a24_e = Fe::from_u64(A24).mul(&e);
        z2 = e.mul(&aa.add(&a24_e));
    }
    cswap(swap, &mut x2, &mut x3);
    cswap(swap, &mut z2, &mut z3);
    x2.mul(&z2.invert()).to_bytes()
}

/// Public key for a 32-byte secret seed: the u-coordinate of the clamped
/// scalar times the base point (RFC 7748 §6.1).
pub fn x25519_public(seed: &[u8; 32]) -> [u8; 32] {
    scalar_mult(&clamp(seed), &BASEPOINT_U)
}

/// Diffie-Hellman: shared secret from our 32-byte secret and the peer's
/// 32-byte public u-coordinate (RFC 7748 §6.1).
pub fn x25519(secret: &[u8; 32], peer_public: &[u8; 32]) -> SharedSecret {
    SharedSecret(scalar_mult(&clamp(secret), peer_public))
}

#[cfg(test)]
mod tests_x25519 {
    use super::*;

    fn unhex(s: &str) -> [u8; 32] {
        let b = s.as_bytes();
        let mut o = [0u8; 32];
        for i in 0..32 {
            let hi = (b[i * 2] as char).to_digit(16).unwrap() as u8;
            let lo = (b[i * 2 + 1] as char).to_digit(16).unwrap() as u8;
            o[i] = (hi << 4) | lo;
        }
        o
    }

    /// RFC 7748 §5.2 test vector 1.
    #[test]
    fn rfc7748_5_2_vector1() {
        let k = unhex("a546e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449ac4");
        let u = unhex("e6db6867583030db3594c1a424b15f7c726624ec26b3353b10a903a6d0ab1c4c");
        let expect = unhex("c3da55379de9c6908e94ea4df28d084f32eccf03491c71f754b4075577a28552");
        assert_eq!(scalar_mult(&clamp(&k), &u), expect);
    }

    /// RFC 7748 §5.2 test vector 2.
    #[test]
    fn rfc7748_5_2_vector2() {
        let k = unhex("4b66e9d4d1b4673c5ad22691957d6af5c11b6421e0ea01d42ca4169e7918ba0d");
        let u = unhex("e5210f12786811d3f4b7959d0538ae2c31dbe7106fc03c3efc4cd549c715a493");
        let expect = unhex("95cbde9476e8907d7aade45cb4b873f88b595a68799fa152e6f8f7647aac7957");
        assert_eq!(scalar_mult(&clamp(&k), &u), expect);
    }

    /// RFC 7748 §5.2 iterated result, one iteration from k = u = 9.
    #[test]
    fn rfc7748_5_2_iterated_once() {
        let k = {
            let mut k = [0u8; 32];
            k[0] = 9;
            k
        };
        let expect = unhex("422c8e7a6227d7bca1350b3e2bb7279f7897b87bb6854b783c60e80311ae3079");
        assert_eq!(x25519_public(&k), expect);
    }

    /// RFC 7748 §6.1 Diffie-Hellman exchange vector: fixed public keys and
    /// the shared secret, verified from both directions.
    #[test]
    fn rfc7748_6_1_exchange() {
        let alice_seed = unhex("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a");
        let bob_seed = unhex("5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb");
        let alice_pub_expect =
            unhex("8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a");
        let bob_pub_expect =
            unhex("de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f");
        let shared_expect =
            unhex("4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742");

        let alice_pub = x25519_public(&alice_seed);
        let bob_pub = x25519_public(&bob_seed);
        assert_eq!(alice_pub, alice_pub_expect, "alice public");
        assert_eq!(bob_pub, bob_pub_expect, "bob public");
        assert_eq!(x25519(&alice_seed, &bob_pub).as_bytes(), &shared_expect);
        assert_eq!(x25519(&bob_seed, &alice_pub).as_bytes(), &shared_expect);
    }
}
