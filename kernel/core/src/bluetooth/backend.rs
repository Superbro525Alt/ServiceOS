//! Kernel-side contract for a Bluetooth (BR/EDR) device backend.
//!
//! Mirrors the [`crate::network::backend::PacketBackend`] house pattern: the
//! kernel never touches the HCI transport; a platform crate provides an
//! `Arc<dyn BluetoothBackend>` implementation and exchanges encoded HCI
//! command packets (built by the platform pure layer) for encoded HCI event
//! packets.
//!
//! Contract shape: the backend accepts encoded HCI command octets, delivers
//! encoded HCI event octets, and reports the transport's link-level
//! capabilities. The kernel stays wire-format-agnostic beyond framing.
//!
//! UNTESTED WITHOUT HARDWARE: no in-tree platform implements this trait yet
//! (row ~88 lands the contract plus the pure protocol layers; qemu-virtio
//! exposes no Bluetooth controller and default boots must not probe for
//! one). Validation would require a real BR/EDR controller (or an
//! instrumented HCI model) plus the userspace wiring from roadmap rows
//! 101/102.

/// Error surface for Bluetooth backend calls.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothError {
    /// Command queue full or controller busy; retry after `poll`.
    Busy,
    /// Controller rejected the operation (bad parameters or unsupported
    /// command).
    Unsupported,
    /// Event queue empty; no encoded event packet available.
    QueueEmpty,
    /// Caller buffer smaller than the pending event packet.
    BufferTooSmall,
    /// Target device class outside the bounded connectable set
    /// (keyboard / audio) supported by this stack slice.
    InvalidClass,
}

/// Link-level capability flags reported by the backend.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BluetoothCapabilities {
    /// Inquiry scan (discoverable) configurable.
    pub inquiry_scan: bool,
    /// BR/EDR ACL connection establishment supported.
    pub acl_connections: bool,
}

/// Kernel-side Bluetooth device contract.
pub trait BluetoothBackend: Send + Sync {
    /// Capability snapshot; the kernel uses it to gate feature paths.
    fn capabilities(&self) -> BluetoothCapabilities;

    /// Submits one encoded HCI command packet to the controller.
    fn submit_command(&self, packet: &[u8]) -> Result<(), BluetoothError>;

    /// Receives the next encoded HCI event packet into `buffer`, returning
    /// its length. `Err(QueueEmpty)` when nothing is pending.
    fn next_event(&self, buffer: &mut [u8]) -> Result<usize, BluetoothError>;

    /// Drives the transport (IRQ fallback / polling mode); `true` when at
    /// least one event became pending.
    fn poll(&self) -> bool;
}
