use alloc::boxed::Box;
use alloc::sync::Arc;
use spin::{Mutex, Once};

use serviceos_abi::{PacketInterfaceBackend, PacketInterfaceInfo, PacketInterfaceLinkState};
use serviceos_kernel_core::network::{PacketBackend, PacketInterfaceError};
use virtio_drivers::{device::net::VirtIONetRaw, transport::DeviceType};

use crate::dtb::VirtioMmioDevice;
use crate::virtio::{KernelHal, VirtioTransport, discover};

const NETWORK_QUEUE_SIZE: usize = 8;
const NETWORK_BUFFER_BYTES: usize = 1536;
const MAX_RECEIVE_QUEUE: usize = 32;
// Header (10-byte legacy or 12-byte modern virtio-net header) + payload.
const TX_BUFFER_BYTES: usize = NETWORK_BUFFER_BYTES + 32;
// One pending slot per possible in-flight TX chain (the queue has 8
// descriptors and can_send gates at 2 free, so at most 7 chains can ever be
// outstanding); the raw driver gives us back the token when the device
// completes, and we retire the slot in poll().
const TX_PENDING_SLOTS: usize = NETWORK_QUEUE_SIZE;

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
    let mut device =
        VirtIONetRaw::<KernelHal, VirtioTransport, NETWORK_QUEUE_SIZE>::new(transport).ok()?;
    let mac = device.mac_address();

    // Pre-post the receive ring with our own heap buffers (the raw driver has
    // no internal buffer management; tokens are handed out in submission
    // order, exactly like the upstream VirtIONet wrapper asserts).
    const NONE_RX: Option<Box<[u8; NETWORK_BUFFER_BYTES]>> = None;
    let mut rx_buffers = [NONE_RX; NETWORK_QUEUE_SIZE];
    for (index, slot) in rx_buffers.iter_mut().enumerate() {
        let mut buffer = Box::new([0u8; NETWORK_BUFFER_BYTES]);
        // SAFETY: the heap buffer stays owned by rx_buffers and is not moved
        // or read until receive_complete retires its token.
        let token = unsafe { device.receive_begin(&mut buffer[..]) }.ok()?;
        if token != index as u16 {
            return None;
        }
        *slot = Some(buffer);
    }

    let backend = Arc::new(VirtioPacketBackend::new(device, rx_buffers, mac));
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
    device: VirtIONetRaw<KernelHal, VirtioTransport, NETWORK_QUEUE_SIZE>,
    rx_buffers: [Option<Box<[u8; NETWORK_BUFFER_BYTES]>>; NETWORK_QUEUE_SIZE],
    tx_pending: [Option<TxPending>; TX_PENDING_SLOTS],
    mac: [u8; 6],
    mtu: u32,
    receive_queue: ReceiveQueue,
    rx_packets: u64,
    tx_packets: u64,
    dropped_packets: u64,
}

/// A transmit handed to the device whose completion has not been reaped yet.
/// The heap buffer keeps the descriptor-alias memory alive and unmodified
/// until `transmit_complete` retires the token.
struct TxPending {
    buffer: Box<[u8; TX_BUFFER_BYTES]>,
    token: u16,
    total_len: usize,
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
        device: VirtIONetRaw<KernelHal, VirtioTransport, NETWORK_QUEUE_SIZE>,
        rx_buffers: [Option<Box<[u8; NETWORK_BUFFER_BYTES]>>; NETWORK_QUEUE_SIZE],
        mac: [u8; 6],
    ) -> Self {
        Self {
            state: Mutex::new(VirtioPacketState {
                device,
                rx_buffers,
                tx_pending: [const { None }; TX_PENDING_SLOTS],
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

impl VirtioPacketState {
    /// Retire every TX chain the device has completed. Called from poll()
    /// (executor loop) and before each submit so descriptors free promptly.
    fn reap_transmits(&mut self) {
        while let Some(token) = self.device.poll_transmit() {
            let Some(slot) = self
                .tx_pending
                .iter()
                .position(|pending| matches!(pending, Some(p) if p.token == token))
            else {
                // Front used element matches no outstanding token: stop rather
                // than spin on a queue the device desynchronized.
                return;
            };
            let Some(pending) = self.tx_pending[slot].take() else {
                return;
            };
            let buffer = pending.buffer;
            // SAFETY: the same heap slice (full extent) that transmit_begin
            // shared with the device; the device finished with it by the time
            // its token reached the used ring front.
            if unsafe {
                self.device
                    .transmit_complete(token, &buffer[..pending.total_len])
            }
            .is_err()
            {
                self.dropped_packets = self.dropped_packets.saturating_add(1);
                // The token was not consumed by the device ring; leave the
                // used element in place and stop this reap pass.
                return;
            }
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
        state.reap_transmits();

        // Non-blocking submit: the blocking VirtIONet::send path spins at EL1
        // inside the syscall (virtio-drivers add_notify_wait_pop) until the
        // DEVICE writes the used ring — a device-side TX stall froze the whole
        // guest permanently. Here a stall simply leaves descriptors in flight
        // and returns Busy; the ring doorbell retries later and completions
        // are reaped in poll().
        if !state.device.can_send() {
            return Err(PacketInterfaceError::Busy);
        }
        let Some(slot) = state.tx_pending.iter().position(Option::is_none) else {
            return Err(PacketInterfaceError::Busy);
        };

        let mut buffer = Box::new([0u8; TX_BUFFER_BYTES]);
        let header_len = state
            .device
            .fill_buffer_header(&mut buffer[..])
            .map_err(|_| PacketInterfaceError::Busy)?;
        buffer[header_len..header_len + frame.len()].copy_from_slice(frame);
        let total_len = header_len + frame.len();
        // SAFETY: the buffer is owned by the pending slot from here on and is
        // neither read nor modified until transmit_complete retires the token.
        let token = unsafe { state.device.transmit_begin(&buffer[..total_len]) }
            .map_err(|_| PacketInterfaceError::Busy)?;

        state.tx_pending[slot] = Some(TxPending {
            buffer,
            token,
            total_len,
        });
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

        // RX drain (poll-based, no IRQ dependency): hand each completed
        // buffer to the receive queue, then re-post it to the device.
        while let Some(token) = state.device.poll_receive() {
            if token as usize >= NETWORK_QUEUE_SIZE {
                break;
            }
            let Some(mut buffer) = state.rx_buffers[token as usize].take() else {
                break;
            };
            // SAFETY: the same full slice receive_begin shared with the
            // device; the token is at the used-ring front, so the device is
            // done writing it.
            let completed = unsafe { state.device.receive_complete(token, &mut buffer[..]) };
            match completed {
                Ok((header_len, packet_len)) => {
                    if state.receive_queue.is_empty() {
                        became_ready = true;
                    }
                    let end = (header_len + packet_len).min(NETWORK_BUFFER_BYTES);
                    if state
                        .receive_queue
                        .push_copy(&buffer[header_len..end])
                        .is_ok()
                    {
                        state.rx_packets = state.rx_packets.saturating_add(1);
                    } else {
                        state.dropped_packets = state.dropped_packets.saturating_add(1);
                    }
                }
                Err(_) => {
                    state.dropped_packets = state.dropped_packets.saturating_add(1);
                    // Buffer lost with its descriptor; stop this drain pass.
                    break;
                }
            }
            // SAFETY: the heap buffer stays owned by rx_buffers until its new
            // token completes.
            match unsafe { state.device.receive_begin(&mut buffer[..]) } {
                Ok(new_token) if (new_token as usize) < NETWORK_QUEUE_SIZE => {
                    state.rx_buffers[new_token as usize] = Some(buffer);
                }
                _ => {
                    state.dropped_packets = state.dropped_packets.saturating_add(1);
                }
            }
        }

        state.reap_transmits();
        became_ready
    }
}
