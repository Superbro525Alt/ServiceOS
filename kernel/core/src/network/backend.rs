use alloc::sync::Arc;
use serviceos_abi::PacketInterfaceInfo;
use spin::Mutex;

use crate::network::ring::{self, PageFrameStorage, RingStorage};

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
                    let length =
                        unsafe { ring::frame_len_at(storage, self.slot_count, head_before) };
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

/// Kernel side of the negotiated shared TX ring — the TX mirror of
/// [`SharedRxRing`]. The network-service is the single producer: it writes
/// outbound frames into memory-object-backed slots through its own mapping,
/// publishes them by advancing the ring head, and rings the doorbell
/// (`PacketInterfaceTxRingFlush`, syscall 54). The kernel is the single
/// consumer: it drains pending slots into the backend transmit path.
///
/// Copy strategy: the kernel COPIES each slot payload into the driver-owned
/// transmit buffer (`VirtIONet::send` requires a driver-allocated TxBuffer,
/// so mapping a userspace page directly into a virtio descriptor is not
/// possible with the current virtio-drivers API). The eliminated copy is the
/// per-frame IPC/syscall copy, exactly mirroring the RX-side win; slot
/// lifecycle keeps every frame IN USE until its transmit completes — the
/// tail counter advances only after `backend.transmit` succeeds.
pub(crate) struct SharedTxRing {
    storage: Mutex<PageFrameStorage>,
    slot_count: usize,
}

// SAFETY: cross-context access coordinates through the ring's head/tail
// protocol; page pointers are dereferenced only inside RingStorage impls.
unsafe impl Send for SharedTxRing {}
unsafe impl Sync for SharedTxRing {}

impl SharedTxRing {
    /// Drain every pending published frame into the backend. Returns the
    /// number of frames transmitted. A frame whose slot cannot be read or
    /// whose length is invalid is discarded (tail advanced without zero-copy
    /// credit); `Busy` from the backend stops the drain with the remaining
    /// frames still pending for the next doorbell.
    fn flush(&self, backend: &dyn PacketBackend) -> usize {
        let mut scratch = [0u8; ring::RING_SLOT_DATA_BYTES];
        let mut transmitted = 0usize;
        loop {
            let next = {
                // SAFETY: single-consumer access over this object's own
                // pages; the kernel-side lock serializes kernel consumers
                // only (the userspace producer coordinates via head/tail).
                let mut guard = self.storage.lock();
                let storage = &mut *guard;
                // SAFETY: see the consumer-exclusivity note above.
                let head = unsafe { ring::load_head(storage) };
                // SAFETY: see the consumer-exclusivity note above.
                let tail = unsafe { ring::load_tail(storage) };
                if tail >= head {
                    break;
                }
                match unsafe { ring::frame_len_at(storage, self.slot_count, tail) } {
                    Some(length) => {
                        let index = ring::slot_of(tail, self.slot_count);
                        // SAFETY: bounded copy of one published slot payload.
                        unsafe {
                            storage.copy_out(ring::slot_data_offset(index), &mut scratch[..length])
                        };
                        Some((tail, length))
                    }
                    // Unusable slot: retire it so one corrupt length field
                    // can never wedge the ring (no zero-copy credit for a
                    // frame that never parsed).
                    None => {
                        unsafe { ring::commit_consumed(storage, tail, 0) };
                        continue;
                    }
                }
            };
            let Some((sequence, length)) = next else {
                break;
            };
            match backend.transmit(&scratch[..length]) {
                Ok(()) => {
                    // Slot stays in use until completion: only now does the
                    // consumer cursor advance past it, banking the zero-copy
                    // credit (tx-copies-avoided / bytes saved).
                    // SAFETY: single-consumer commit over the header page.
                    unsafe { self.commit(sequence, length) };
                    transmitted += 1;
                }
                Err(PacketInterfaceError::BufferTooSmall) => {
                    // Malformed frame for this backend: drop it rather than
                    // retrying forever.
                    // SAFETY: single-consumer commit over the header page.
                    unsafe { self.commit(sequence, 0) };
                }
                Err(_) => break,
            }
        }
        transmitted
    }

    /// Commit consumption of `sequence` after its transmit completed. The
    /// lock scope is explicit so no page access races the drain loop.
    ///
    /// # Safety
    /// Single-consumer per ring; storage must cover the header page.
    unsafe fn commit(&self, sequence: u64, length: usize) {
        let mut guard = self.storage.lock();
        // SAFETY: caller guarantees single-consumer access.
        unsafe { ring::commit_consumed(&mut *guard, sequence, length) };
    }
}

pub struct PacketInterfaceObject {
    backend: Arc<dyn PacketBackend>,
    shared_ring: Mutex<Option<Arc<SharedRxRing>>>,
    shared_tx_ring: Mutex<Option<Arc<SharedTxRing>>>,
}

impl PacketInterfaceObject {
    pub fn new(backend: Arc<dyn PacketBackend>) -> Self {
        Self {
            backend,
            shared_ring: Mutex::new(None),
            shared_tx_ring: Mutex::new(None),
        }
    }

    pub fn info(&self) -> PacketInterfaceInfo {
        self.backend.info()
    }

    /// Legacy copied TX path. With a shared TX ring attached this first
    /// opportunistically drains any backlog (so a producer that stopped
    /// doorbelling cannot wedge queued frames behind new traffic), then
    /// hands `frame` to the backend exactly as before.
    pub fn transmit(&self, frame: &[u8]) -> Result<(), PacketInterfaceError> {
        self.flush_transmits();
        self.backend.transmit(frame)
    }

    /// Attach the kernel side of a negotiated shared TX ring. Idempotent:
    /// repeat negotiation returns the existing ring.
    pub(crate) fn attach_shared_tx_ring(
        &self,
        storage: PageFrameStorage,
        slot_count: usize,
    ) -> Arc<SharedTxRing> {
        let mut guard = self.shared_tx_ring.lock();
        if let Some(existing) = guard.as_ref() {
            return Arc::clone(existing);
        }
        let created = Arc::new(SharedTxRing {
            storage: Mutex::new(storage),
            slot_count,
        });
        *guard = Some(Arc::clone(&created));
        created
    }

    pub(crate) fn has_shared_tx_ring(&self) -> bool {
        self.shared_tx_ring.lock().is_some()
    }

    /// Doorbell drain: transmit every frame the service has published into
    /// the shared TX ring. No-op on interfaces that never negotiated one.
    pub fn flush_transmits(&self) -> usize {
        match self.shared_tx_ring.lock().as_ref() {
            Some(ring) => ring.flush(self.backend.as_ref()),
            None => 0,
        }
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
    use crate::memory::{PAGE_SIZE_BYTES, PhysicalAddress};
    use crate::network::ring::{self, PageFrameStorage, RingStorage};
    use alloc::{vec, vec::Vec};
    use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use serviceos_abi::{PacketInterfaceBackend, PacketInterfaceInfo, PacketInterfaceLinkState};

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

    /// TX-side backend: records every accepted frame and can be armed to
    /// reject the next N transmits with `Busy` (the virtio queue-full shape).
    struct TxRecordingBackend {
        sent: Mutex<Vec<(u8, usize)>>,
        busy_remaining: AtomicUsize,
    }

    impl TxRecordingBackend {
        fn new() -> Self {
            Self {
                sent: Mutex::new(Vec::new()),
                busy_remaining: AtomicUsize::new(0),
            }
        }

        fn arm_busy(&self, count: usize) {
            self.busy_remaining.store(count, Ordering::SeqCst);
        }
    }

    impl PacketBackend for TxRecordingBackend {
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

        fn transmit(&self, frame: &[u8]) -> Result<(), PacketInterfaceError> {
            // Consume an armed Busy slot without going negative.
            loop {
                let current = self.busy_remaining.load(Ordering::SeqCst);
                if current == 0 {
                    break;
                }
                if self
                    .busy_remaining
                    .compare_exchange(current, current - 1, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    return Err(PacketInterfaceError::Busy);
                }
            }
            self.sent.lock().push((frame[0], frame.len()));
            Ok(())
        }

        fn receive(&self, _buffer: &mut [u8]) -> Result<usize, PacketInterfaceError> {
            Err(PacketInterfaceError::QueueEmpty)
        }

        fn poll(&self) -> bool {
            false
        }
    }

    fn tx_marker(marker: u8, len: usize) -> Vec<u8> {
        let mut frame = Vec::with_capacity(len);
        for _ in 0..len {
            frame.push(marker);
        }
        frame
    }

    #[test]
    fn tx_ring_flush_drains_fifo_across_wraparound_with_zero_copy_credit() {
        let slot_count = 3;
        let storage = ring_storage(slot_count);
        // Producer and observer views over the same physical pages (the
        // service's mapping in production): free-running sequences wrap
        // past slot_count.
        let mut producer = PageFrameStorage {
            frames: storage.frames.clone(),
        };
        let reader = PageFrameStorage {
            frames: storage.frames.clone(),
        };
        let backend = Arc::new(TxRecordingBackend::new());
        let object = PacketInterfaceObject::new(Arc::clone(&backend) as Arc<dyn PacketBackend>);
        assert!(!object.has_shared_tx_ring());
        object.attach_shared_tx_ring(storage, slot_count);
        assert!(object.has_shared_tx_ring());

        const FRAME_LEN: usize = 24;
        // Burst phase: fill the ring to exact capacity (the service only
        // ever publishes while `head - tail < slot_count`, so this is the
        // deepest legal backlog) and drain it with ONE doorbell.
        for marker in 0u8..3u8 {
            // SAFETY(test): single-producer access over the whole image.
            let _ = unsafe { ring::push(&mut producer, slot_count, &tx_marker(marker, FRAME_LEN)) };
        }
        assert_eq!(object.flush_transmits(), 3, "one doorbell drains the burst");

        // Steady phase: publish + doorbell per frame so sequences free-run
        // past slot_count — the TX wraparound shape (7 total through 3
        // slots, nothing dropped because the consumer keeps pace).
        for marker in 3u8..7u8 {
            // SAFETY(test): single-producer access over the whole image.
            let _ = unsafe { ring::push(&mut producer, slot_count, &tx_marker(marker, FRAME_LEN)) };
            assert_eq!(object.flush_transmits(), 1);
        }

        let sent = backend.sent.lock();
        assert_eq!(
            sent.iter().map(|(marker, _)| *marker).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4, 5, 6],
            "FIFO order survives slot wraparound"
        );
        assert!(sent.iter().all(|(_, len)| *len == FRAME_LEN));
        drop(sent);

        // Ring is fully consumed: tail caught up to head, zero-copy stats
        // banked once per completed transmit (tx-copies-avoided).
        // SAFETY(test): reads over the leaked test image backing frames.
        let word = |offset: usize| unsafe { reader.load_u64(offset) };
        assert_eq!(word(ring::OFF_HEAD), 7);
        assert_eq!(word(ring::OFF_TAIL), 7);
        assert_eq!(word(ring::OFF_COPIES_AVOIDED), 7);
        assert_eq!(word(ring::OFF_BYTES_SAVED), 7 * FRAME_LEN as u64);

        // Legacy copied path keeps working alongside the ring.
        assert!(object.transmit(&tx_marker(9, FRAME_LEN)).is_ok());
        assert_eq!(backend_sent_len(&backend), 8);
    }

    #[test]
    fn tx_ring_slot_stays_in_use_until_transmit_completes() {
        let slot_count = 2;
        let storage = ring_storage(slot_count);
        let mut producer = PageFrameStorage {
            frames: storage.frames.clone(),
        };
        let reader = PageFrameStorage {
            frames: storage.frames.clone(),
        };
        let backend = Arc::new(TxRecordingBackend::new());
        backend.arm_busy(1);
        let object = PacketInterfaceObject::new(Arc::clone(&backend) as Arc<dyn PacketBackend>);
        object.attach_shared_tx_ring(storage, slot_count);

        // SAFETY(test): single-producer access over the whole image.
        let _ = unsafe { ring::push(&mut producer, slot_count, &tx_marker(5, 16)) };

        // First doorbell hits the Busy device: NOTHING may be consumed —
        // the published slot stays in use (head advanced, tail untouched,
        // no zero-copy credit banked).
        assert_eq!(object.flush_transmits(), 0);
        assert_eq!(backend_sent_len(&backend), 0);
        // SAFETY(test): reads over the leaked test image backing frames.
        let word = |offset: usize| unsafe { reader.load_u64(offset) };
        assert_eq!(word(ring::OFF_HEAD), 1, "producer published");
        assert_eq!(word(ring::OFF_TAIL), 0, "slot in use until completion");
        assert_eq!(word(ring::OFF_COPIES_AVOIDED), 0, "no credit yet");

        // Retry doorbell once the device accepts: same sequence completes,
        // exactly one copy lands, credit banks once.
        assert_eq!(object.flush_transmits(), 1);
        assert_eq!(backend_sent_len(&backend), 1);
        assert_eq!(backend.sent.lock()[0].0, 5);
        assert_eq!(word(ring::OFF_TAIL), 1);
        assert_eq!(word(ring::OFF_COPIES_AVOIDED), 1);
        assert_eq!(word(ring::OFF_BYTES_SAVED), 16);

        // Draining an empty ring is a clean no-op.
        assert_eq!(object.flush_transmits(), 0);
    }

    #[test]
    fn tx_without_ring_keeps_legacy_copied_path_only() {
        let backend = Arc::new(TxRecordingBackend::new());
        let object = PacketInterfaceObject::new(Arc::clone(&backend) as Arc<dyn PacketBackend>);
        assert!(!object.has_shared_tx_ring());
        assert_eq!(object.flush_transmits(), 0, "no ring: doorbell is a no-op");
        assert!(object.transmit(&tx_marker(1, 8)).is_ok());
        assert_eq!(backend_sent_len(&backend), 1, "copied TX fallback intact");
    }

    fn backend_sent_len(backend: &TxRecordingBackend) -> usize {
        backend.sent.lock().len()
    }
}
