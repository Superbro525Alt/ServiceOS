//! Raspberry Pi 5 network bring-up.
//!
//! The Pi 5 has no supported NIC in-tree: the onboard BCM43455 (PCIe-attached
//! FullMAC wifi on BCM2712 boards) and the RP1 USB/GbE path both need drivers
//! that do not exist in this tree, so the network-backed service graph stays
//! open. The graphical service graph, however, hard-requires a kernel packet
//! interface: network-service (a required dependency of desktop-shell) exits
//! before registration when its startup packet handle is not a real packet
//! object, which would wedge the whole graphical boot. This module mints a
//! NULL packet backend (link Down, zero MAC, transmit accepted-and-counted,
//! receive always `QueueEmpty`) so network-service comes up, reports
//! `network-interface-ready`, and honestly reports no address: DHCP discovery
//! transmits into the sink and the configuration state stays Pending, so the
//! selftest never runs and the service sits Ready-but-unaddressed. This is
//! the same shape of honest absence the x86 platforms show when no NIC is
//! detected. UNTESTED WITHOUT HARDWARE — nothing here can be exercised until
//! a real Pi 5 boots the image (raspi5 is ManualDeploy in QEMU). Real
//! DWGE/PCIe NIC support replaces this backend behind the same object.

use alloc::sync::Arc;

use serviceos_abi::{PacketInterfaceInfo, PacketInterfaceLinkState};
use serviceos_kernel_core::network::{PacketBackend, PacketInterfaceError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkStatus {
    pub implemented: bool,
}

pub const fn status() -> NetworkStatus {
    NetworkStatus { implemented: false }
}

/// MTU reported to consumers; matches the Ethernet default so smoltcp
/// interface math stays conventional even though no frame ever flows.
const NULL_LINK_MTU: u32 = 1500;

/// Null packet interface: no PHY, no queue. Transmits are accepted and
/// counted (discarded) so protocol drivers make forward progress instead of
/// error-spinning; receives always report an empty queue.
pub struct NullPacketBackend {
    state: spin::Mutex<NullLinkState>,
}

#[derive(Default)]
struct NullLinkState {
    tx_packets: u64,
    tx_bytes: u64,
    dropped_packets: u64,
}

impl NullPacketBackend {
    fn new() -> Self {
        Self {
            state: spin::Mutex::new(NullLinkState::default()),
        }
    }
}

impl PacketBackend for NullPacketBackend {
    fn info(&self) -> PacketInterfaceInfo {
        let state = self.state.lock();
        PacketInterfaceInfo {
            backend: serviceos_abi::PacketInterfaceBackend::Unknown as u32,
            link_state: PacketInterfaceLinkState::Down as u32,
            mtu: NULL_LINK_MTU,
            rx_ready: 0,
            mac: [0; 6],
            reserved: [0; 2],
            rx_packets: 0,
            tx_packets: state.tx_packets,
            dropped_packets: state.dropped_packets,
        }
    }

    fn transmit(&self, frame: &[u8]) -> Result<(), PacketInterfaceError> {
        let mut state = self.state.lock();
        if frame.is_empty() {
            state.dropped_packets = state.dropped_packets.saturating_add(1);
            return Ok(());
        }
        state.tx_packets = state.tx_packets.saturating_add(1);
        state.tx_bytes = state.tx_bytes.saturating_add(frame.len() as u64);
        Ok(())
    }

    fn receive(&self, _buffer: &mut [u8]) -> Result<usize, PacketInterfaceError> {
        Err(PacketInterfaceError::QueueEmpty)
    }

    fn poll(&self) -> bool {
        false
    }
}

/// Builds the null packet-interface backend for the graphical service graph.
pub fn null_packet_backend() -> Arc<dyn PacketBackend> {
    Arc::new(NullPacketBackend::new())
}
