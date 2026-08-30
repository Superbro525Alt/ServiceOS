//! Kernel-side contract for a wireless (Wi-Fi) device backend.
//!
//! Mirrors the [`crate::network::backend::PacketBackend`] house pattern: the
//! kernel never touches device registers; a platform crate provides an
//! `Arc<dyn WirelessBackend>` implementation and exchanges *self-describing
//! control envelopes* (CFG80211-style attribute TLVs, see the platform pure
//! layer) through submit/receive channels.
//!
//! Contract shape: the backend accepts encoded command envelopes, delivers
//! encoded event envelopes (scan results, link-status changes), and reports
//! its feature set. The kernel stays wire-format-agnostic beyond framing.
//!
//! UNTESTED WITHOUT HARDWARE: no in-tree platform implements this trait yet
//! (row ~88 lands the contract plus the pure protocol layers; qemu-virtio
//! exposes no virtio-wlan device and default boots must not probe for one).
//! Validation would require a real 802.11 NIC (or an instrumented
//! virtio-wlan model) plus the userspace wiring from roadmap rows 101/102.

/// Error surface for wireless backend calls.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WirelessError {
    /// Command queue full or device busy; retry after `poll`.
    Busy,
    /// Device rejected the operation (bad parameters or unsupported command).
    Unsupported,
    /// Event queue empty; no encoded event available.
    QueueEmpty,
    /// Caller buffer smaller than the pending event envelope.
    BufferTooSmall,
    /// Link is not in a state that permits the operation (e.g. join while
    /// already connected).
    InvalidState,
}

/// Device capability flags reported by the backend.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WirelessCapabilities {
    /// Background scan (off-channel probe requests) supported.
    pub background_scan: bool,
    /// WPA2-PSK (CCMP) supported.
    pub wpa2_psk: bool,
    /// WPA3-SAE supported (key management beyond the pure placeholder).
    pub wpa3_sae: bool,
}

/// Kernel-side wireless device contract.
pub trait WirelessBackend: Send + Sync {
    /// Capability snapshot; the kernel uses it to gate feature paths.
    fn capabilities(&self) -> WirelessCapabilities;

    /// Submits one encoded command envelope (scan/join/auth/associate,
    /// built by the platform pure layer) to the device.
    fn submit_command(&self, envelope: &[u8]) -> Result<(), WirelessError>;

    /// Receives the next encoded event envelope (scan result record,
    /// handshake frames, link-status change) into `buffer`, returning its
    /// length. `Err(QueueEmpty)` when nothing is pending.
    fn next_event(&self, buffer: &mut [u8]) -> Result<usize, WirelessError>;

    /// Drives the device (IRQ fallback / polling mode); `true` when at
    /// least one event became pending.
    fn poll(&self) -> bool;
}
