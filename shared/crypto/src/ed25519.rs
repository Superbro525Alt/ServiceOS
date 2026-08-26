//! Ed25519 signatures (RFC 8032), pure `core`.
//!
//! - `public_key` / `sign` are for host-side tooling and test fixtures.
//! - `verify` is the OS-side path used by package-service feed verification.
//!
//! Deviations from constant-time orthodoxy (documented): signing uses the
//! seed-derived scalar directly with masked-select scalar multiplication
//! (no Montgomery ladder); verification early-rejects on malformed public
//! inputs. No secret-dependent branches exist outside signing.

use crate::point::{decompress, Point};
use crate::scalar::Scalar;
use crate::sha512;

/// Derive the 32-byte compressed public key for a 32-byte seed.
pub fn public_key(seed: &[u8; 32]) -> [u8; 32] {
    let h = sha512::digest(&[seed]);
    let a = clamp(&h);
    Point::base().mul_scalar(&Scalar(a)).compress()
}

/// Sign `message` with a 32-byte seed. Returns R || S (64 bytes).
pub fn sign(seed: &[u8; 32], message: &[u8]) -> [u8; 64] {
    let h = sha512::digest(&[seed]);
    let a = clamp(&h);
    let public = Point::base().mul_scalar(&Scalar(a)).compress();
    let r = Scalar::reduce_wide(&sha512::digest(&[&h[32..64], message]));
    let big_r = Point::base().mul_scalar(&r).compress();
    let k = Scalar::reduce_wide(&sha512::digest(&[&big_r, &public, message]));
    // S = (r + k * a) mod L
    let s = Scalar::mul_add(&k, &Scalar(a), &r);
    let mut sig = [0u8; 64];
    sig[..32].copy_from_slice(&big_r);
    sig[32..].copy_from_slice(&s.0);
    sig
}

/// Verify `(message, signature)` against a compressed public key.
pub fn verify(public_key: &[u8; 32], message: &[u8], signature: &[u8; 64]) -> bool {
    let Some(point_a) = decompress(public_key) else {
        return false;
    };
    let mut r_bytes = [0u8; 32];
    r_bytes.copy_from_slice(&signature[..32]);
    let Some(big_r) = decompress(&r_bytes) else {
        return false;
    };
    let mut s_bytes = [0u8; 32];
    s_bytes.copy_from_slice(&signature[32..]);
    // RFC 8032 step 3: reject non-canonical S.
    if !Scalar::is_canonical(&s_bytes) {
        return false;
    }
    let k = Scalar::reduce_wide(&sha512::digest(&[
        &r_bytes,
        public_key,
        message,
    ]));
    // Check [S]B == R + [k]A.
    let lhs = Point::base().mul_scalar(&Scalar(s_bytes));
    let rhs = big_r.add(&point_a.mul_scalar(&k));
    Point::eq(&lhs, &rhs)
}

fn clamp(h: &[u8; 64]) -> [u8; 32] {
    let mut a = [0u8; 32];
    a.copy_from_slice(&h[..32]);
    a[0] &= 248;
    a[31] &= 127;
    a[31] |= 64;
    a
}
