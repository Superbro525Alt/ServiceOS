use alloc::sync::Arc;
use spin::{Mutex, Once};

use serviceos_abi::{PacketInterfaceBackend, PacketInterfaceInfo, PacketInterfaceLinkState};
use serviceos_kernel_core::network::{PacketBackend, PacketInterfaceError};
use virtio_drivers::{device::net::VirtIONet, transport::DeviceType};

use crate::dtb::VirtioMmioDevice;
use crate::virtio::{KernelHal, VirtioTransport, discover};

const NETWORK_QUEUE_SIZE: usize = 8;
const NETWORK_BUFFER_BYTES: usize = 1536;
const MAX_RECEIVE_QUEUE: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkBringupSummary {
    pub backend: PacketInterfaceBackend,
    pub mmio_base: u64,
    pub irq: u32,
    pub mtu: u32,
    pub mac: [u8; 6],
}

pub fn initialize(devices: &[VirtioMmioDevice]) -> Option<Arc<dyn PacketBackend>> {
    let (discovered, transport) = discover(devices, DeviceType::Network).into_iter().next()?;
    let device = VirtIONet::<KernelHal, VirtioTransport, NETWORK_QUEUE_SIZE>::new(
        transport,
        NETWORK_BUFFER_BYTES,
    )
    .ok()?;
    let mac = device.mac_address();
    let backend = Arc::new(VirtioPacketBackend::new(device, mac));
    let _ = BRINGUP_SUMMARY.call_once(|| NetworkBringupSummary {
        backend: PacketInterfaceBackend::VirtioPci,
        mmio_base: discovered.mmio_base,
        irq: discovered.irq,
        mtu: 1500,
        mac,
    });
    Some(backend)
}

pub fn bringup_summary() -> Option<NetworkBringupSummary> {
    BRINGUP_SUMMARY.get().copied()
}

static BRINGUP_SUMMARY: Once<NetworkBringupSummary> = Once::new();

struct VirtioPacketBackend {
    state: Mutex<VirtioPacketState>,
}

struct VirtioPacketState {
    device: VirtIONet<KernelHal, VirtioTransport, NETWORK_QUEUE_SIZE>,
    mac: [u8; 6],
    mtu: u32,
    receive_queue: ReceiveQueue,
    rx_packets: u64,
    tx_packets: u64,
    dropped_packets: u64,
}

struct ReceiveQueue {
    slots: [[u8; NETWORK_BUFFER_BYTES]; MAX_RECEIVE_QUEUE],
    lengths: [usize; MAX_RECEIVE_QUEUE],
    head: usize,
    tail: usize,
    len: usize,
}

impl ReceiveQueue {
    const fn new() -> Self {
        Self {
            slots: [[0; NETWORK_BUFFER_BYTES]; MAX_RECEIVE_QUEUE],
            lengths: [0; MAX_RECEIVE_QUEUE],
            head: 0,
            tail: 0,
            len: 0,
        }
    }

    const fn is_empty(&self) -> bool {
        self.len == 0
    }

    const fn is_full(&self) -> bool {
        self.len == MAX_RECEIVE_QUEUE
    }

    fn pop_front_into(&mut self, buffer: &mut [u8]) -> Result<usize, PacketInterfaceError> {
        if self.is_empty() {
            return Err(PacketInterfaceError::QueueEmpty);
        }

        let frame_len = self.lengths[self.head];
        if frame_len > buffer.len() {
            return Err(PacketInterfaceError::BufferTooSmall);
        }

        buffer[..frame_len].copy_from_slice(&self.slots[self.head][..frame_len]);
        self.lengths[self.head] = 0;
        self.head = (self.head + 1) % MAX_RECEIVE_QUEUE;
        self.len -= 1;
        Ok(frame_len)
    }

    fn push_copy(&mut self, frame: &[u8]) -> Result<(), PacketInterfaceError> {
        if frame.len() > NETWORK_BUFFER_BYTES {
            return Err(PacketInterfaceError::BufferTooSmall);
        }

        if self.is_full() {
            self.head = (self.head + 1) % MAX_RECEIVE_QUEUE;
            self.len -= 1;
        }

        self.slots[self.tail][..frame.len()].copy_from_slice(frame);
        self.lengths[self.tail] = frame.len();
        self.tail = (self.tail + 1) % MAX_RECEIVE_QUEUE;
        self.len += 1;
        Ok(())
    }
}

impl VirtioPacketBackend {
    fn new(
        device: VirtIONet<KernelHal, VirtioTransport, NETWORK_QUEUE_SIZE>,
        mac: [u8; 6],
    ) -> Self {
        Self {
            state: Mutex::new(VirtioPacketState {
                device,
                mac,
                mtu: 1500,
                receive_queue: ReceiveQueue::new(),
                rx_packets: 0,
                tx_packets: 0,
                dropped_packets: 0,
            }),
        }
    }
}

impl PacketBackend for VirtioPacketBackend {
    fn info(&self) -> PacketInterfaceInfo {
        let state = self.state.lock();
        PacketInterfaceInfo {
            backend: PacketInterfaceBackend::VirtioPci as u32,
            link_state: PacketInterfaceLinkState::Up as u32,
            mtu: state.mtu,
            rx_ready: u32::from(!state.receive_queue.is_empty()),
            mac: state.mac,
            reserved: [0; 2],
            rx_packets: state.rx_packets,
            tx_packets: state.tx_packets,
            dropped_packets: state.dropped_packets,
        }
    }

    fn transmit(&self, frame: &[u8]) -> Result<(), PacketInterfaceError> {
        if frame.is_empty() || frame.len() > NETWORK_BUFFER_BYTES {
            return Err(PacketInterfaceError::BufferTooSmall);
        }

        let mut state = self.state.lock();
        if !state.device.can_send() {
            return Err(PacketInterfaceError::Busy);
        }

        let mut tx = state.device.new_tx_buffer(frame.len());
        tx.packet_mut().copy_from_slice(frame);
        state
            .device
            .send(tx)
            .map_err(|_| PacketInterfaceError::Busy)?;
        state.tx_packets = state.tx_packets.saturating_add(1);
        Ok(())
    }

    fn receive(&self, buffer: &mut [u8]) -> Result<usize, PacketInterfaceError> {
        let mut state = self.state.lock();
        state.receive_queue.pop_front_into(buffer)
    }

    fn poll(&self) -> bool {
        let mut state = self.state.lock();
        let mut became_ready = false;

        while state.device.can_recv() {
            match state.device.receive() {
                Ok(rx) => {
                    if state.receive_queue.is_empty() {
                        became_ready = true;
                    }
                    let queue_result = state.receive_queue.push_copy(rx.packet());
                    let _ = state.device.recycle_rx_buffer(rx);
                    if queue_result.is_err() {
                        state.dropped_packets = state.dropped_packets.saturating_add(1);
                        continue;
                    }
                    state.rx_packets = state.rx_packets.saturating_add(1);
                }
                Err(_) => break,
            }
        }

        became_ready
    }
}
