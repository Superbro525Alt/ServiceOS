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
    /// Receive one backend frame into the next shared slot. `Ok(None)` means
    /// the backend had nothing even after a poll retry.
    fn receive_into_slot(
        &self,
        receive: impl Fn(&mut [u8]) -> Result<usize, PacketInterfaceError> + Copy,
        poll_once: impl Fn(),
    ) -> Result<Option<usize>, PacketInterfaceError> {
        let attempt = || -> Result<Option<usize>, PacketInterfaceError> {
            // SAFETY: single-producer access over this object's own pages;
            // the kernel-side lock serializes kernel producers only (the
            // userspace consumer coordinates via the head/tail protocol).
            #[allow(unused_mut)]
            let mut guard = self.storage.lock();
            // SAFETY: see the producer-exclusivity note above.
            let storage = &mut *guard;            let published =
                unsafe { ring::push_fill(storage, self.slot_count, |slot| receive(slot)) }?;
            Ok(published.map(|sequence| sequence as usize))
        };

        match attempt()? {
            Some(published) => Ok(Some(published)),
            None => {
                poll_once();
                attempt()
            }
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
                    published.map(Ok).unwrap_or(Err(PacketInterfaceError::QueueEmpty))
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
