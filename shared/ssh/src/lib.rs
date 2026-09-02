//! SSH-2.0 transport layer for ServiceOS — a pure, fixed-capacity, host-tested
//! state-machine library (no heap, no RNG, no I/O).
//!
//! Scope (honest): RFC 4253 version exchange + binary packet protocol
//! (unencrypted pre-KEX framing and the `chacha20-poly1305@openssh.com` AEAD
//! packet format), RFC 8731 `curve25519-sha256` key exchange with the RFC 4253
//! §7.2 SSH key derivation over SHA-256, and `ssh-ed25519` host-key blobs with
//! signature verification of the exchange hash (RFC 4253 §8, §7). The server
//! role is the operator surface; a minimal client role exists solely as a
//! test helper for the in-process handshake harness.
//!
//! Established-state services (server side): RFC 4252 user authentication
//! with the `password` method only (`auth` — credentials are parked for the
//! host's own verifier, three failures disconnect), and one RFC 4254
//! interactive session channel (`channel` — pty-req + shell, windowed data
//! both ways, EOF/close semantics). The session bridge to a real shell lives
//! with the sshd pump in network-service.
//!
//! Explicitly NOT implemented (later waves): rekeying, compression,
//! known_hosts host-key trust, publickey/keyboard-interactive auth, port
//! forwarding, exec/subsystem channels. Unimplemented packet types are
//! answered with SSH_MSG_UNIMPLEMENTED or an honest DISCONNECT (a rekey
//! KEXINIT answers PROTOCOL_ERROR).
//!
//! Trust gap (documented, deliberate): the transport verifies the host-key
//! signature over the exchange hash cryptographically, but performs no
//! host-key authentication — any host key is accepted as long as it signs
//! correctly, so a man-in-the-middle that terminates both halves is not
//! detected this wave. A known_hosts store is future work.
//!
//! Purity rules: every buffer is a fixed-capacity array, all arithmetic is
//! bounds-checked, no allocation, no randomness (cookies, ephemeral seeds and
//! padding bytes are caller-supplied or zero-filled — see the module docs of
//! `packet` and `kex`), and secrets are only ever compared constant-time via
//! `serviceos-crypto`.

#![cfg_attr(not(test), no_std)]
#![allow(dead_code)]

pub mod auth;
pub mod channel;
pub mod error;
pub mod hostkey;
pub mod kex;
pub mod negotiate;
pub mod packet;
pub mod transport;
pub mod version;
pub mod wire;

#[cfg(test)]
pub(crate) mod testkit;

pub use error::{DisconnectReason, Fail};
pub use transport::{Feed, Role, SshTransport, State};

/// Crate-wide software version reported in the SSH identification string.
pub const SOFTWARE_VERSION: &str = "ServiceOS_0.1.0";
