//! Shared RX packet ring between the kernel packet-interface object and the
//! network service (S7 packet-buffer sharing increment).
//!
//! The ring image is a memory object whose backing pages are mapped into the
//! consumer (network-service) address space; both sides talk through the
//! same physical pages, so frames are consumed IN PLACE with no per-frame
//! IPC copy.
//!
//! Layout (little-endian):
//!
//! ```text
//! page 0, offset 0   ring header:
//!     0  u32  magic (RING_MAGIC)
//!     4  u32  version
//!     8  u32  slot_count
//!     12 u32  slot_data_bytes
//!     16 u64  head           producer-owned free-running sequence counter
//!     24 u64  tail           consumer-owned free-running sequence counter
//!     32 u64  frames_pushed
//!     40 u64  copies_avoided
//!     48 u64  bytes_saved
//!     56 u64  dropped
//! ```
//!
//! Each slot owns exactly one whole page (`slot i` lives in page `i + 1`)
//! so a slot can never straddle a physical page boundary: `[len: u64]`
//! followed by `RING_SLOT_DATA_BYTES` of frame data. Sequence selection is
//! `counter % slot_count`, so both counters free-run and wraparound needs no
//! extra modulo state.
//!
//! Protocol: the kernel (single producer) fills the next slot and publishes
//! it by advancing `head`; the network-service (single consumer) parses the
//! slot in place through its mapping and commits by advancing `tail`. When
//! the ring is full the OLDEST frame is retired, matching the legacy
//! ReceiveQueue overflow policy.

use alloc::sync::Arc;

use crate::memory::{PhysicalAddress, PAGE_SIZE_BYTES};

pub const RING_MAGIC: u32 = 0x534f_5258;
pub const RING_VERSION: u32 = 1;
pub const RING_HEADER_BYTES: usize = 64;
pub const RING_SLOT_DATA_BYTES: usize = 1536;
pub const RING_DEFAULT_SLOTS: usize = 16;

pub const OFF_MAGIC: usize = 0;
pub const OFF_VERSION: usize = 4;
pub const OFF_SLOTS: usize = 8;
pub const OFF_HEAD: usize = 16;
pub const OFF_TAIL: usize = 24;
pub const OFF_FRAMES_PUSHED: usize = 32;
pub const OFF_COPIES_AVOIDED: usize = 40;
pub const OFF_BYTES_SAVED: usize = 48;
pub const OFF_DROPPED: usize = 56;

/// Total image size: one header page plus one page per slot.
pub const fn ring_total_bytes(slot_count: usize) -> usize {
    (slot_count + 1) * PAGE_SIZE_BYTES as usize
}

const fn slot_page(slot_index: usize) -> usize {
    slot_index + 1
}

/// Flat-image byte offset of a slot's length field.
pub const fn slot_len_offset(slot_index: usize) -> usize {
    slot_page(slot_index) * PAGE_SIZE_BYTES as usize
}

/// Flat-image byte offset of a slot's frame-data region.
pub const fn slot_data_offset(slot_index: usize) -> usize {
    slot_len_offset(slot_index) + 8
}

/// Byte-storage abstraction so the ring logic is exercisable by host unit
/// tests against a plain slice while the kernel uses physically-backed
/// memory-object pages.
pub trait RingStorage {
    /// # Safety
    /// `offset..offset + data.len()` must lie inside the ring image.
    unsafe fn copy_in(&mut self, offset: usize, data: &[u8]);
    /// # Safety
    /// `offset..offset + out.len()` must lie inside the ring image.
    unsafe fn copy_out(&self, offset: usize, out: &mut [u8]);
    /// # Safety
    /// `offset..offset + 8` must lie inside the ring image.
    unsafe fn load_u64(&self, offset: usize) -> u64 {
        let mut bytes = [0u8; 8];
        // SAFETY: caller upheld bounds for the 8-byte read.
        unsafe { self.copy_out(offset, &mut bytes) };
        u64::from_le_bytes(bytes)
    }
    /// # Safety
    /// `offset..offset + 8` must lie inside the ring image.
    unsafe fn store_u64(&mut self, offset: usize, value: u64) {
        let bytes = value.to_le_bytes();
        // SAFETY: caller upheld bounds for the 8-byte write.
        unsafe { self.copy_in(offset, &bytes) };
    }
    /// Mutable view over one slot's frame-data region (exactly
    /// `RING_SLOT_DATA_BYTES`, always within that slot's single page).
    ///
    /// # Safety
    /// `slot_index` must be below the ring's configured slot count.
    unsafe fn slot_payload(&mut self, slot_index: usize) -> &mut [u8];
}

pub struct SliceStorage<'a>(pub &'a mut [u8]);

impl RingStorage for SliceStorage<'_> {
    unsafe fn copy_in(&mut self, offset: usize, data: &[u8]) {
        // SAFETY: caller upheld bounds.
        unsafe {
            core::ptr::copy_nonoverlapping(
                data.as_ptr(),
                self.0.as_mut_ptr().add(offset),
                data.len(),
            );
        }
    }

    unsafe fn copy_out(&self, offset: usize, out: &mut [u8]) {
        // SAFETY: caller upheld bounds.
        unsafe {
            core::ptr::copy_nonoverlapping(
                self.0.as_ptr().add(offset),
                out.as_mut_ptr(),
                out.len(),
            );
        }
    }

    unsafe fn load_u64(&self, offset: usize) -> u64 {
        // SAFETY: caller upheld bounds.
        unsafe { core::ptr::read_unaligned(self.0.as_ptr().add(offset) as *const u64) }
    }

    unsafe fn store_u64(&mut self, offset: usize, value: u64) {
        // SAFETY: caller upheld bounds.
        unsafe {
            core::ptr::write_unaligned(self.0.as_mut_ptr().add(offset) as *mut u64, value)
        }
    }

    unsafe fn slot_payload(&mut self, slot_index: usize) -> &mut [u8] {
        let base = slot_data_offset(slot_index);
        // SAFETY: slot region lies inside the image by construction.
        unsafe {
            core::slice::from_raw_parts_mut(self.0.as_mut_ptr().add(base), RING_SLOT_DATA_BYTES)
        }
    }
}

/// Physically page-backed storage shared with userspace mappers of the same
/// memory object. Frame 0 backs the header; frame `i + 1` backs slot `i`.
pub struct PageFrameStorage {
    pub frames: Arc<[PhysicalAddress]>,
}

impl RingStorage for PageFrameStorage {
    unsafe fn copy_in(&mut self, offset: usize, data: &[u8]) {
        let page_size = PAGE_SIZE_BYTES as usize;
        let mut done = 0;
        while done < data.len() {
            let position = offset + done;
            let page = position / page_size;
            let in_page = position % page_size;
            let chunk = (page_size - in_page).min(data.len() - done);
            let base = self.frames[page].as_u64() as usize + in_page;
            // SAFETY: chunk stays inside page `page` of this object's frames.
            unsafe {
                core::ptr::copy_nonoverlapping(data[done..].as_ptr(), base as *mut u8, chunk);
            }
            done += chunk;
        }
    }

    unsafe fn copy_out(&self, offset: usize, out: &mut [u8]) {
        let page_size = PAGE_SIZE_BYTES as usize;
        let mut done = 0;
        while done < out.len() {
            let position = offset + done;
            let page = position / page_size;
            let in_page = position % page_size;
            let chunk = (page_size - in_page).min(out.len() - done);
            let base = self.frames[page].as_u64() as usize + in_page;
            // SAFETY: chunk stays inside page `page` of this object's frames.
            unsafe {
                core::ptr::copy_nonoverlapping(base as *const u8, out[done..].as_mut_ptr(), chunk);
            }
            done += chunk;
        }
    }

    unsafe fn load_u64(&self, offset: usize) -> u64 {
        let mut bytes = [0u8; 8];
        // SAFETY: bounded read inside the image.
        unsafe { self.copy_out(offset, &mut bytes) };
        u64::from_le_bytes(bytes)
    }

    unsafe fn store_u64(&mut self, offset: usize, value: u64) {
        let bytes = value.to_le_bytes();
        // SAFETY: bounded write inside the image.
        unsafe { self.copy_in(offset, &bytes) };
    }

    unsafe fn slot_payload(&mut self, slot_index: usize) -> &mut [u8] {
        let frame = self.frames[slot_page(slot_index)].as_u64() as *mut u8;
        // SAFETY: the slot's own page; data region starts after the length
        // field and never leaves that page.
        unsafe { core::slice::from_raw_parts_mut(frame.add(8), RING_SLOT_DATA_BYTES) }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PushError {
    FrameTooLarge,
}

#[cfg(test)]
fn read_word<S: RingStorage>(storage: &S, offset: usize) -> u32 {
    let mut bytes = [0u8; 4];
    // SAFETY: bounded header read at a compile-time constant offset.
    unsafe { storage.copy_out(offset, &mut bytes) };
    u32::from_le_bytes(bytes)
}

/// Initialize the ring header in freshly-created (zeroed) shared storage.
pub fn init<S: RingStorage>(storage: &mut S, slot_count: usize) {
    // SAFETY: compile-time-constant offsets inside the header page.
    unsafe {
        storage.copy_in(OFF_MAGIC, &RING_MAGIC.to_le_bytes());
        storage.copy_in(OFF_VERSION, &RING_VERSION.to_le_bytes());
        storage.copy_in(OFF_SLOTS, &(slot_count as u32).to_le_bytes());
        storage.copy_in(12, &(RING_SLOT_DATA_BYTES as u32).to_le_bytes());
        for offset in [
            OFF_HEAD,
            OFF_TAIL,
            OFF_FRAMES_PUSHED,
            OFF_COPIES_AVOIDED,
            OFF_BYTES_SAVED,
            OFF_DROPPED,
        ] {
            storage.store_u64(offset, 0);
        }
    }
}

/// Validate header words read back from mapped storage.
pub const fn validate_header(
    magic: u32,
    version: u32,
    slot_count: u32,
    expected_slots: usize,
) -> bool {
    magic == RING_MAGIC && version == RING_VERSION && slot_count as usize == expected_slots
}

/// Producer view of the free-running head counter.
///
/// # Safety
/// Storage must cover the header page.
pub unsafe fn load_head<S: RingStorage>(storage: &S) -> u64 {
    unsafe { storage.load_u64(OFF_HEAD) }
}

/// Consumer view of the free-running tail counter.
///
/// # Safety
/// Storage must cover the header page.
pub unsafe fn load_tail<S: RingStorage>(storage: &S) -> u64 {
    unsafe { storage.load_u64(OFF_TAIL) }
}

/// Number of unconsumed frames currently visible.
pub const fn pending(head: u64, tail: u64) -> u64 {
    head.wrapping_sub(tail)
}

/// Consumer peek: length of the frame with sequence `sequence`, if it has
/// been published (`sequence < head`). Callers drive `sequence` from their
/// own tail cursor, so no cross-side tail read happens here.
///
/// # Safety
/// Storage must cover the queried slot page.
pub unsafe fn frame_len_at<S: RingStorage>(
    storage: &S,
    slot_count: usize,
    sequence: u64,
) -> Option<usize> {
    let head = unsafe { storage.load_u64(OFF_HEAD) };
    if sequence >= head {
        return None;
    }
    let index = (sequence % slot_count as u64) as usize;
    let len = unsafe { storage.load_u64(slot_len_offset(index)) } as usize;
    (len > 0 && len <= RING_SLOT_DATA_BYTES).then_some(len)
}

/// Consumer commit: mark `sequence` consumed and accumulate the zero-copy
/// statistics carried in the header.
///
/// # Safety
/// Single consumer per ring; storage must cover the header page.
pub unsafe fn commit_consumed<S: RingStorage>(
    storage: &mut S,
    sequence: u64,
    length: usize,
) {
    debug_assert!(length <= RING_SLOT_DATA_BYTES);
    // SAFETY: consumer owns the tail counter; stat accumulation is
    // single-consumer.
    unsafe {
        storage.store_u64(OFF_TAIL, sequence.wrapping_add(1));
        let avoided = storage.load_u64(OFF_COPIES_AVOIDED);
        storage.store_u64(OFF_COPIES_AVOIDED, avoided.wrapping_add(1));
        let saved = storage.load_u64(OFF_BYTES_SAVED);
        storage.store_u64(OFF_BYTES_SAVED, saved.wrapping_add(length as u64));
    }
}

pub const fn slot_of(sequence: u64, slot_count: usize) -> usize {
    (sequence % slot_count as u64) as usize
}

/// Push one frame as the producer. When the ring is full the OLDEST frame is
/// dropped (matching the legacy ReceiveQueue overflow policy) and the
/// `dropped` counter bumps. Returns the assigned sequence number.
///
/// # Safety
/// Single producer per ring; storage must cover the full image.
pub unsafe fn push<S: RingStorage>(
    storage: &mut S,
    slot_count: usize,
    frame: &[u8],
) -> Result<u64, PushError> {
    if frame.len() > RING_SLOT_DATA_BYTES {
        return Err(PushError::FrameTooLarge);
    }
    // SAFETY: single-producer in-place fill; the closure copies the frame.
    unsafe {
        push_fill(storage, slot_count, |slot| {
            slot[..frame.len()].copy_from_slice(frame);
            Ok::<usize, PushError>(frame.len())
        })
    }
    .map(|published| published.expect("frame always publishes"))
}

/// Producer entry with an in-place fill callback: the next pending sequence
/// is claimed (with drop-oldest when full), the slot payload is handed to
/// `fill`, and the frame is published only when `fill` reports a length.
/// `Ok(None)` means nothing was filled (head unchanged, nothing published).
///
/// # Safety
/// Single producer per ring; storage must cover the full image.
pub unsafe fn push_fill<S: RingStorage, E>(
    storage: &mut S,
    slot_count: usize,
    fill: impl FnOnce(&mut [u8]) -> Result<usize, E>,
) -> Result<Option<u64>, E> {
    let head = unsafe { storage.load_u64(OFF_HEAD) };
    let tail = unsafe { storage.load_u64(OFF_TAIL) };

    if head.wrapping_sub(tail) >= slot_count as u64 {
        // Full: retire the oldest frame (sequence `tail`, which occupies the
        // same slot this push is about to overwrite) to make room.
        unsafe {
            storage.store_u64(OFF_TAIL, tail.wrapping_add(1));
            let dropped = storage.load_u64(OFF_DROPPED);
            storage.store_u64(OFF_DROPPED, dropped.wrapping_add(1));
        }
    }

    let index = slot_of(head, slot_count);
    let length = unsafe { fill(storage.slot_payload(index))? };

    // SAFETY: publish the filled slot and advance the producer counter.
    unsafe {
        storage.store_u64(slot_len_offset(index), length as u64);
        storage.store_u64(OFF_HEAD, head.wrapping_add(1));
        let pushed = storage.load_u64(OFF_FRAMES_PUSHED);
        storage.store_u64(OFF_FRAMES_PUSHED, pushed.wrapping_add(1));
    }
    Ok(Some(head))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    /// Host-side stand-in for the shared image: same layout, plain Vec.
    struct HostRing {
        bytes: Vec<u8>,
        slots: usize,
    }

    impl HostRing {
        fn new(slots: usize) -> Self {
            let mut ring = Self {
                bytes: vec![0u8; ring_total_bytes(slots)],
                slots,
            };
            init(&mut SliceStorage(&mut ring.bytes), slots);
            ring
        }

        fn push(&mut self, frame: &[u8]) -> Result<u64, PushError> {
            // SAFETY: test-only single-producer access over the whole image.
            unsafe { push(&mut SliceStorage(&mut self.bytes), self.slots, frame) }
        }

        fn commit(&mut self, sequence: u64, length: usize) {
            // SAFETY: test-only single-consumer access over the whole image.
            unsafe {
                commit_consumed(&mut SliceStorage(&mut self.bytes), sequence, length)
            }
        }

        fn counter(&self, offset: usize) -> u64 {
            u64::from_le_bytes(self.bytes[offset..offset + 8].try_into().unwrap())
        }

        fn head(&self) -> u64 {
            self.counter(OFF_HEAD)
        }

        fn tail(&self) -> u64 {
            self.counter(OFF_TAIL)
        }

        fn frame_len(&self, sequence: u64) -> Option<usize> {
            if sequence >= self.head() {
                return None;
            }
            let base = slot_len_offset(slot_of(sequence, self.slots));
            let len =
                u64::from_le_bytes(self.bytes[base..base + 8].try_into().unwrap()) as usize;
            (len > 0 && len <= RING_SLOT_DATA_BYTES).then_some(len)
        }

        /// In-place claim straight from the image (the shape a mapped
        /// consumer uses): locate the slot and borrow its payload.
        fn claim_data(&self, sequence: u64) -> Option<&[u8]> {
            let index = slot_of(sequence, self.slots);
            if self.frame_len(sequence).is_none() && sequence >= self.head() {
                return None;
            }
            let base = slot_len_offset(index);
            let len =
                u64::from_le_bytes(self.bytes[base..base + 8].try_into().unwrap()) as usize;
            if len == 0 || len > RING_SLOT_DATA_BYTES {
                return None;
            }
            Some(&self.bytes[base + 8..base + 8 + len])
        }
    }

    #[test]
    fn layout_constants_are_stable() {
        assert_eq!(ring_total_bytes(0) / PAGE_SIZE_BYTES as usize, 1);
        assert_eq!(
            ring_total_bytes(RING_DEFAULT_SLOTS),
            (RING_DEFAULT_SLOTS + 1) * PAGE_SIZE_BYTES as usize
        );
        // Every slot owns one whole page: length field at the page start.
        for index in 0..RING_DEFAULT_SLOTS {
            assert_eq!(slot_len_offset(index), (index + 1) * PAGE_SIZE_BYTES as usize);
            assert!(slot_data_offset(index) + RING_SLOT_DATA_BYTES
                <= (index + 2) * PAGE_SIZE_BYTES as usize);
        }
    }

    #[test]
    fn init_writes_discoverable_header() {
        let ring = HostRing::new(4);
        let mut storage_view = ring.bytes.clone();
        let words = {
            let storage = SliceStorage(&mut storage_view);
            (
                read_word(&storage, OFF_MAGIC),
                read_word(&storage, OFF_VERSION),
                read_word(&storage, OFF_SLOTS),
            )
        };
        assert!(validate_header(words.0, words.1, words.2, 4));
        assert_eq!(words.0, RING_MAGIC);
        assert_eq!(ring.head(), 0);
        assert_eq!(ring.tail(), 0);
    }

    #[test]
    fn fifo_order_preserved_across_wraparound_with_oldest_dropped() {
        let mut ring = HostRing::new(4);
        let mut sequences = Vec::new();
        for marker in 0u8..9u8 {
            sequences.push(ring.push(&marker_frame(marker, 32)).unwrap());
        }
        // 9 frames through 4 slots: the oldest 5 were retired.
        assert_eq!(ring.counter(OFF_DROPPED), 5);
        assert_eq!(ring.counter(OFF_FRAMES_PUSHED), 9);

        for (step, sequence) in sequences.iter().enumerate().skip(5) {
            let data = ring.claim_data(*sequence).expect("pending frame");
            assert_eq!(data[0], step as u8, "FIFO order across wraparound");
            assert_eq!(data.len(), 32);
            ring.commit(*sequence, data.len());
        }
        assert_eq!(ring.tail(), 9);
        assert_eq!(ring.counter(OFF_COPIES_AVOIDED), 4);
        assert_eq!(ring.counter(OFF_BYTES_SAVED), 4 * 32);
    }

    #[test]
    fn steady_state_consume_as_you_go_free_runs_counters() {
        let mut ring = HostRing::new(3);
        for round in 0u64..(2 * 3 + 2) {
            let payload = [(round as u8); 10];
            let sequence = ring.push(&payload).unwrap();
            assert_eq!(sequence, round, "sequences free-run past slot_count");
            assert_eq!(pending(ring.head(), ring.tail()), 1);
            let claimed = ring.claim_data(sequence).unwrap();
            assert_eq!(claimed, &payload[..]);
            ring.commit(sequence, claimed.len());
            assert_eq!(ring.tail(), round + 1);
        }
        assert_eq!(ring.counter(OFF_DROPPED), 0);
        assert_eq!(ring.counter(OFF_BYTES_SAVED), (2 * 3 + 2) * 10);
    }

    #[test]
    fn oversize_frames_are_rejected_without_corruption() {
        let mut ring = HostRing::new(2);
        let ok = ring.push(&[1u8; 16]).unwrap();
        assert_eq!(
            ring.push(&[0u8; RING_SLOT_DATA_BYTES + 1]),
            Err(PushError::FrameTooLarge)
        );
        assert_eq!(ring.claim_data(ok), Some(&[1u8; 16][..]));
        assert_eq!(ring.counter(OFF_DROPPED), 0);
    }

    fn marker_frame(marker: u8, len: usize) -> Vec<u8> {
        let mut frame = Vec::with_capacity(len);
        for _ in 0..len {
            frame.push(marker);
        }
        frame
    }

    /// Golden Ethernet+IPv4+UDP frame fixture parsed IN PLACE from the mapped
    /// ring layout — the same zero-copy shape the network-service consumer
    /// uses against its memory-object mapping.
    #[test]
    fn golden_frame_parses_in_place_from_mapped_layout() {
        // eth: dst 02:00:00:00:00:02 <- src 02:00:00:00:00:01, type 0x0800;
        // ipv4: v4/IHL 5, total_len 37, id 0x1234, ttl 64, proto 17 (UDP),
        //       src 10.0.2.15, dst 10.0.2.2 (checksum zeroed; the in-place
        //       parse under test does not verify checksums);
        // udp: sport 5353 dport 41453 len 17 cksum 0 + payload "zero-copy".
        let golden: [u8; 51] = [
            0x02, 0x00, 0x00, 0x00, 0x00, 0x02, 0x02, 0x00, 0x00, 0x00, 0x00, 0x01, 0x08, 0x00,
            0x45, 0x00, 0x00, 0x25, 0x12, 0x34, 0x00, 0x00, 0x40, 0x11, 0x00, 0x00, 10, 0, 2, 15,
            10, 0, 2, 2, 0x14, 0xe9, 0xa1, 0xed, 0x00, 0x11, 0x00, 0x00, b'z', b'e', b'r', b'o',
            b'-', b'c', b'o', b'p', b'y',
        ];
        assert_eq!(golden.len(), 14 + 20 + 8 + 9);

        let mut ring = HostRing::new(RING_DEFAULT_SLOTS);
        let sequence = ring.push(&golden).unwrap();

        // In-place parse from the image itself, exactly how a mapped
        // consumer locates and reads the slot (no intermediate copy).
        let index = slot_of(sequence, RING_DEFAULT_SLOTS);
        let base = slot_len_offset(index);
        let mapped_len =
            u64::from_le_bytes(ring.bytes[base..base + 8].try_into().unwrap()) as usize;
        assert_eq!(mapped_len, golden.len());
        let mapped = &ring.bytes[base + 8..base + 8 + mapped_len];

        assert_eq!(u16::from_be_bytes([mapped[12], mapped[13]]), 0x0800, "ethertype");
        assert_eq!(&mapped[26..30], &[10, 0, 2, 15], "ipv4 src");
        assert_eq!(&mapped[30..34], &[10, 0, 2, 2], "ipv4 dst");
        assert_eq!(mapped[23], 17, "protocol udp");
        assert_eq!(u16::from_be_bytes([mapped[34], mapped[35]]), 5353, "src port");
        assert_eq!(u16::from_be_bytes([mapped[36], mapped[37]]), 41453, "dst port");
        assert_eq!(&mapped[42..], b"zero-copy");

        ring.commit(sequence, mapped_len);
        assert_eq!(ring.counter(OFF_COPIES_AVOIDED), 1);
        assert_eq!(ring.counter(OFF_BYTES_SAVED), golden.len() as u64);
    }
}
