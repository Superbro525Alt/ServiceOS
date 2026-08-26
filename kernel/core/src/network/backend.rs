use alloc::sync::Arc;
use spin::Mutex;
use serviceos_abi::PacketInterfaceInfo;

use crate::network::ring::{self, PageFrameStorage};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PacketInterfaceError {
    QueueEmpty,
    BufferTooSmall,
    Busy,
    Unsupported,
}

pub trait PacketBackend: Send + Sync {
    fn info(&self) -> PacketInterfaceInfo;
    fn transmit(&self, frame: &[u8]) -> Result<(), PacketInterfaceError>;
    fn receive(&self, buffer: &mut [u8]) -> Result<usize, PacketInterfaceError>;
    fn poll(&self) -> bool;
}

/// Kernel side of the negotiated shared RX ring. Once attached, received
/// frames are filled directly into memory-object-backed slots (a single copy
/// out of the backend's internal queue) instead of being copied into caller
/// IPC buffers; the network-service consumer claims slots in place through
/// its own mapping.
pub(crate) struct SharedRxRing {
    storage: Mutex<PageFrameStorage>,
    slot_count: usize,
}

// SAFETY: cross-context access coordinates through the ring's head/tail
// protocol; page pointers are dereferenced only inside RingStorage impls.
unsafe impl Send for SharedRxRing {}
unsafe impl Sync for SharedRxRing {}

impl SharedRxRing {
    /// Receive one backend frame into the next shared slot. Returns the
    /// published frame's LENGTH (never the raw ring sequence — sequence 0
    /// would be indistinguishable from "no data" through the length-based
    /// receive contract). `Ok(None)` means the backend had nothing even
    /// after a poll retry.
    fn receive_into_slot(
        &self,
        receive: impl Fn(&mut [u8]) -> Result<usize, PacketInterfaceError> + Copy,
        poll_once: impl Fn(),
    ) -> Result<Option<usize>, PacketInterfaceError> {
        let attempt = || -> Result<Option<usize>, PacketInterfaceError> {
            // SAFETY: single-producer access over this object's own pages;
            // the kernel-side lock serializes kernel producers only (the
            // userspace consumer coordinates via the head/tail protocol).
            let mut guard = self.storage.lock();
            // SAFETY: see the producer-exclusivity note above.
            let storage = &mut *guard;
            let head_before = unsafe { ring::load_head(storage) };
            let published =
                unsafe { ring::push_fill(storage, self.slot_count, |slot| receive(slot)) }?;
            match published {
                None => Ok(None),
                Some(_) => {
                    let length = unsafe {
                        ring::frame_len_at(storage, self.slot_count, head_before)
                    };
                    Ok(Some(length.unwrap_or(0)))
                }
            }
        };

        match attempt() {
            Ok(Some(published)) => Ok(Some(published)),
            // Empty backend queue (or nothing published after a fill) is not
            // final: poll the device once and retry, matching the legacy
            // copied-path semantics in `receive_copied`. Without this the
            // shared-ring path depends entirely on the IRQ draining the
            // device, and a missed/unacked interrupt stalls RX forever.
            Ok(None) | Err(PacketInterfaceError::QueueEmpty) => {
                poll_once();
                attempt()
            }
            Err(error) => Err(error),
        }
    }
}

pub struct PacketInterfaceObject {
    backend: Arc<dyn PacketBackend>,
    shared_ring: Mutex<Option<Arc<SharedRxRing>>>,
}

impl PacketInterfaceObject {
    pub fn new(backend: Arc<dyn PacketBackend>) -> Self {
        Self {
            backend,
            shared_ring: Mutex::new(None),
        }
    }

    pub fn info(&self) -> PacketInterfaceInfo {
        self.backend.info()
    }

    pub fn transmit(&self, frame: &[u8]) -> Result<(), PacketInterfaceError> {
        self.backend.transmit(frame)
    }

    /// Attach the kernel side of a negotiated shared RX ring. Idempotent:
    /// repeat negotiation returns the existing ring.
    pub(crate) fn has_shared_ring(&self) -> bool {
        self.shared_ring.lock().is_some()
    }

    pub(crate) fn attach_shared_ring(
        &self,
        storage: PageFrameStorage,
        slot_count: usize,
    ) -> Arc<SharedRxRing> {
        let mut guard = self.shared_ring.lock();
        if let Some(existing) = guard.as_ref() {
            return Arc::clone(existing);
        }
        let created = Arc::new(SharedRxRing {
            storage: Mutex::new(storage),
            slot_count,
        });
        *guard = Some(Arc::clone(&created));
        created
    }

    /// Receive one frame. Without a shared ring this copies into `buffer`
    /// exactly like before (the legacy fallback path). With a ring attached
    /// the frame body lands in the next shared slot instead — a successful
    /// return is a doorbell telling the consumer to claim that sequence from
    /// its own mapping — so the per-frame IPC copy disappears. In ring mode
    /// the contents of `buffer` are untouched and only its length bound
    /// matters for legacy compatibility of the call signature.
    pub fn receive(&self, buffer: &mut [u8]) -> Result<usize, PacketInterfaceError> {
        let shared = self.shared_ring.lock().clone();
        if let Some(shared) = shared.as_ref() {
            let backend = &self.backend;
            return shared
                .receive_into_slot(
                    |slot| backend.receive(slot),
                    || {
                        let _ = backend.poll();
                    },
                )
                .and_then(|published| {
                    published
                        .filter(|length| *length > 0)
                        .map(Ok)
                        .unwrap_or(Err(PacketInterfaceError::QueueEmpty))
                });
        }
        self.receive_copied(buffer)
    }

    fn receive_copied(&self, buffer: &mut [u8]) -> Result<usize, PacketInterfaceError> {
        match self.backend.receive(buffer) {
            Ok(length) => Ok(length),
            Err(PacketInterfaceError::QueueEmpty) => {
                let _ = self.backend.poll();
                self.backend.receive(buffer)
            }
            Err(error) => Err(error),
        }
    }

    pub fn backend(&self) -> Arc<dyn PacketBackend> {
        Arc::clone(&self.backend)
    }
}

#[cfg(test)]
mod shared_ring_tests {
    use super::*;
    use crate::memory::{PhysicalAddress, PAGE_SIZE_BYTES};
    use crate::network::ring::{self, PageFrameStorage};
    use alloc::{vec, vec::Vec};
    use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use serviceos_abi::{
        PacketInterfaceBackend, PacketInterfaceInfo, PacketInterfaceLinkState,
    };

    /// Backend whose receive queue starts empty; a poll arms exactly one
    /// canned frame delivery. Mirrors the virtio device holding completed RX
    /// buffers that only `poll()` drains into the kernel queue.
    struct PollFedBackend {
        remaining: AtomicUsize,
        armed: AtomicBool,
        frame: [u8; 4],
    }

    impl PacketBackend for PollFedBackend {
        fn info(&self) -> PacketInterfaceInfo {
            PacketInterfaceInfo {
                backend: PacketInterfaceBackend::Unknown as u32,
                link_state: PacketInterfaceLinkState::Up as u32,
                mtu: 1500,
                rx_ready: 0,
                mac: [0; 6],
                reserved: [0; 2],
                rx_packets: 0,
                tx_packets: 0,
                dropped_packets: 0,
            }
        }

        fn transmit(&self, _frame: &[u8]) -> Result<(), PacketInterfaceError> {
            Ok(())
        }

        fn receive(&self, buffer: &mut [u8]) -> Result<usize, PacketInterfaceError> {
            if self.frame.len() > buffer.len() {
                return Err(PacketInterfaceError::BufferTooSmall);
            }
            let was_armed = self.armed.swap(false, Ordering::SeqCst);
            let was_remaining = self.remaining.fetch_sub(1, Ordering::SeqCst);
            if was_armed && was_remaining > 0 {
                buffer[..self.frame.len()].copy_from_slice(&self.frame);
                Ok(self.frame.len())
            } else {
                self.remaining.fetch_add(1, Ordering::SeqCst);
                Err(PacketInterfaceError::QueueEmpty)
            }
        }

        fn poll(&self) -> bool {
            self.armed.store(true, Ordering::SeqCst);
            true
        }
    }

    fn ring_storage(slot_count: usize) -> PageFrameStorage {
        let total = ring::ring_total_bytes(slot_count);
        let image = Vec::leak(vec![0u8; total]);
        let frames: Vec<PhysicalAddress> = (0..=slot_count)
            .map(|page| {
                PhysicalAddress::new(image.as_ptr() as u64 + (page as u64) * PAGE_SIZE_BYTES as u64)
            })
            .collect();
        let storage = PageFrameStorage {
            frames: frames.into(),
        };
        let mut mutable = PageFrameStorage {
            frames: storage.frames.clone(),
        };
        ring::init(&mut mutable, slot_count);
        storage
    }

    #[test]
    fn shared_ring_receive_polls_backend_when_queue_empty() {
        let object = PacketInterfaceObject::new(Arc::new(PollFedBackend {
            remaining: AtomicUsize::new(1),
            armed: AtomicBool::new(false),
            frame: [1, 2, 3, 4],
        }));
        object.attach_shared_ring(ring_storage(4), 4);

        let mut buffer = [0u8; 64];
        let received = object.receive(&mut buffer).expect("polled frame must flow");
        assert_eq!(received, 4);
    }

    #[test]
    fn shared_ring_receive_reports_empty_after_poll_retry() {
        // Poll "refills" only while frames remain; once exhausted the second
        // receive attempt finds nothing even after polling and must surface
        // QueueEmpty.
        let object = PacketInterfaceObject::new(Arc::new(PollFedBackend {
            remaining: AtomicUsize::new(1),
            armed: AtomicBool::new(false),
            frame: [9, 9, 9, 9],
        }));
        object.attach_shared_ring(ring_storage(4), 4);

        let mut buffer = [0u8; 64];
        let first = object.receive(&mut buffer).expect("first receive");
        assert_eq!(first, 4);
        assert!(matches!(
            object.receive(&mut buffer),
            Err(PacketInterfaceError::QueueEmpty)
        ));
    }
}
