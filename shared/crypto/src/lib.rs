#![cfg_attr(not(feature = "std-helper"), no_std)]

//! Pure-Rust cryptographic primitives for ServiceOS.
//!
//! Everything here is implemented from scratch against the relevant specs
//! (FIPS 180-4 for SHA-256/SHA-512, RFC 8032 for Ed25519) using only `core`, so the
//! crate builds for the bare-metal userspace targets as well as the host.
//!
//! Constant-time discipline: field arithmetic, scalar reduction, and point
//! selection avoid data-dependent branches on secret-dependent values.
//! Documented deviations: point equality comparison and canonical encoding
//! checks operate on public data; scalar multiplication processes every bit
//! but selects via masked moves rather than a Montgomery ladder; verification
//! early-rejects on malformed (public) inputs.

pub mod chacha20;
pub mod chacha20poly1305;
pub mod ed25519;
mod field;
pub mod hkdf;
pub mod hmac;
#[cfg(feature = "std-helper")]
pub mod host;
pub mod pbkdf2;
mod point;
pub mod poly1305;
mod scalar;
pub mod sha256;
pub mod sha512;
pub mod x25519;
