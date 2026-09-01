use alloc::{boxed::Box, sync::Arc, vec, vec::Vec};
use spin::Mutex;

use crate::{
    memory::{self, PAGE_SIZE_BYTES, PhysicalAddress},
    time::MonotonicInstant,
};

pub struct BootstrapCapabilityObject;

impl BootstrapCapabilityObject {
    pub const fn new() -> Self {
        Self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventStateView {
    pub signaled: bool,
    pub signal_count: u64,
}

pub struct EventObject {
    state: Mutex<EventState>,
}

struct EventState {
    signaled: bool,
    signal_count: u64,
}

impl EventObject {
    pub fn new(signaled: bool) -> Self {
        Self {
            state: Mutex::new(EventState {
                signaled,
                signal_count: 0,
            }),
        }
    }

    pub fn signal(&self) {
        let mut state = self.state.lock();
        state.signaled = true;
        state.signal_count = state.signal_count.saturating_add(1);
    }

    pub fn reset(&self) {
        self.state.lock().signaled = false;
    }

    pub fn snapshot(&self) -> EventStateView {
        let state = self.state.lock();

        EventStateView {
            signaled: state.signaled,
            signal_count: state.signal_count,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimerStateView {
    pub armed: bool,
    pub deadline: Option<MonotonicInstant>,
    pub periodic_interval_ticks: Option<u64>,
}

pub struct TimerObject {
    state: Mutex<TimerState>,
}

struct TimerState {
    armed: bool,
    deadline: Option<MonotonicInstant>,
    periodic_interval_ticks: Option<u64>,
}

impl TimerObject {
    pub fn new(deadline: Option<MonotonicInstant>, periodic_interval_ticks: Option<u64>) -> Self {
        Self {
            state: Mutex::new(TimerState {
                armed: deadline.is_some(),
                deadline,
                periodic_interval_ticks,
            }),
        }
    }

    pub fn arm(&self, deadline: MonotonicInstant, periodic_interval_ticks: Option<u64>) {
        let mut state = self.state.lock();
        state.armed = true;
        state.deadline = Some(deadline);
        state.periodic_interval_ticks = periodic_interval_ticks;
    }

    pub fn disarm(&self) {
        let mut state = self.state.lock();
        state.armed = false;
        state.deadline = None;
        state.periodic_interval_ticks = None;
    }

    pub fn snapshot(&self) -> TimerStateView {
        let state = self.state.lock();

        TimerStateView {
            armed: state.armed,
            deadline: state.deadline,
            periodic_interval_ticks: state.periodic_interval_ticks,
        }
    }
}

/// Device-DMA classification for a memory object. Kernel-internal only:
/// deliberately not part of the shared/abi surface (no repr(C) layout bump).
///
/// - `Unsafe` (default): no device-access guarantee; any attempt to fetch a
///   physical device backing through [`MemoryObject::device_backing`] is a
///   policy violation.
/// - `PagePinned`: every device-visible access stays inside one whole
///   physical page (a ring slot never straddles a page boundary; see the
///   `network/ring.rs` layout rationale).
/// - `Contiguous`: additionally the backing frames are physically
///   contiguous, verified when the backing is materialized.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DmaSafety {
    #[default]
    Unsafe,
    PagePinned,
    Contiguous,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryObjectInfo {
    pub size_bytes: usize,
    pub page_count: usize,
    pub writable: bool,
    pub dma_safety: DmaSafety,
}

pub struct MemoryObject {
    info: MemoryObjectInfo,
    storage: MemoryStorage,
}

enum MemoryStorage {
    ReadOnly(Arc<[u8]>),
    Writable(Mutex<WritableMemoryState>),
}

struct WritableMemoryState {
    backing: WritableMemoryBacking,
}

enum WritableMemoryBacking {
    Linear(Box<[u8]>),
    PageBacked(Arc<[PhysicalAddress]>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryAccessError {
    ReadOnly,
    Busy,
    Unsupported,
    DmaPolicyViolation,
}

impl MemoryObject {
    pub fn new(size_bytes: usize, writable: bool, dma_safety: DmaSafety) -> Self {
        let page_count = size_bytes.div_ceil(4096);
        let zeroed = vec![0u8; size_bytes].into_boxed_slice();

        Self {
            info: MemoryObjectInfo {
                size_bytes,
                page_count,
                writable,
                dma_safety,
            },
            storage: if writable {
                MemoryStorage::Writable(Mutex::new(WritableMemoryState {
                    backing: WritableMemoryBacking::Linear(zeroed),
                }))
            } else {
                MemoryStorage::ReadOnly(Arc::from(zeroed))
            },
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        let size_bytes = bytes.len();
        let page_count = size_bytes.div_ceil(4096);
        Self {
            info: MemoryObjectInfo {
                size_bytes,
                page_count,
                writable: false,
                dma_safety: DmaSafety::Unsafe,
            },
            storage: MemoryStorage::ReadOnly(Arc::from(bytes)),
        }
    }

    pub const fn info(&self) -> MemoryObjectInfo {
        self.info
    }

    pub fn read(&self, offset: usize, destination: &mut [u8]) -> usize {
        match &self.storage {
            MemoryStorage::ReadOnly(bytes) => {
                let Some(source) = bytes.get(offset..) else {
                    return 0;
                };
                let len = source.len().min(destination.len());
                destination[..len].copy_from_slice(&source[..len]);
                len
            }
            MemoryStorage::Writable(bytes) => {
                let bytes = bytes.lock();
                match &bytes.backing {
                    WritableMemoryBacking::Linear(bytes) => {
                        let Some(source) = bytes.get(offset..) else {
                            return 0;
                        };
                        let len = source.len().min(destination.len());
                        destination[..len].copy_from_slice(&source[..len]);
                        len
                    }
                    WritableMemoryBacking::PageBacked(frames) => {
                        read_page_backed(frames, self.info.size_bytes, offset, destination)
                    }
                }
            }
        }
    }

    pub fn write(&self, offset: usize, source: &[u8]) -> Result<usize, MemoryAccessError> {
        let MemoryStorage::Writable(bytes) = &self.storage else {
            return Err(MemoryAccessError::ReadOnly);
        };
        let mut bytes = bytes.lock();
        match &mut bytes.backing {
            WritableMemoryBacking::Linear(bytes) => {
                let Some(destination) = bytes.get_mut(offset..) else {
                    return Ok(0);
                };
                let len = destination.len().min(source.len());
                destination[..len].copy_from_slice(&source[..len]);
                Ok(len)
            }
            WritableMemoryBacking::PageBacked(frames) => Ok(write_page_backed(
                frames,
                self.info.size_bytes,
                offset,
                source,
            )),
        }
    }

    pub fn page_frames(&self) -> Result<Arc<[PhysicalAddress]>, MemoryAccessError> {
        let MemoryStorage::Writable(state) = &self.storage else {
            return Err(MemoryAccessError::ReadOnly);
        };
        let mut state = state.lock();
        let frames = match &state.backing {
            WritableMemoryBacking::PageBacked(frames) => Arc::clone(frames),
            WritableMemoryBacking::Linear(bytes) => {
                let frames = allocate_page_backing(bytes, self.info.page_count)?;
                state.backing = WritableMemoryBacking::PageBacked(Arc::clone(&frames));
                frames
            }
        };
        verify_dma_contiguity(self.info.dma_safety, &frames)?;
        Ok(frames)
    }

    /// The DMA policy gate: fetch the physical backing for device access.
    /// Refuses `Unsafe` objects before any physical surface is produced;
    /// `Contiguous` objects additionally get their physical contiguity
    /// verified at materialization (see [`Self::page_frames`]). The CPU
    /// map-range path is unaffected: it goes through [`Self::page_frames`].
    pub fn device_backing(&self) -> Result<Arc<[PhysicalAddress]>, MemoryAccessError> {
        if let DmaSafety::Unsafe = self.info.dma_safety {
            return Err(MemoryAccessError::DmaPolicyViolation);
        }
        self.page_frames()
    }
}

/// `Contiguous` objects must materialize as one physically contiguous run;
/// anything else is a policy violation (declared-safe-but-discontiguous).
fn verify_dma_contiguity(
    dma_safety: DmaSafety,
    frames: &[PhysicalAddress],
) -> Result<(), MemoryAccessError> {
    if let DmaSafety::Contiguous = dma_safety {
        if !frames_are_contiguous(frames) {
            return Err(MemoryAccessError::DmaPolicyViolation);
        }
    }
    Ok(())
}

/// True when `frames` form one physically contiguous ascending run. Single-
/// and zero-frame slices are trivially contiguous.
pub(super) fn frames_are_contiguous(frames: &[PhysicalAddress]) -> bool {
    frames
        .windows(2)
        .all(|pair| pair[1].as_u64() == pair[0].as_u64() + PAGE_SIZE_BYTES as u64)
}

fn allocate_page_backing(
    bytes: &[u8],
    page_count: usize,
) -> Result<Arc<[PhysicalAddress]>, MemoryAccessError> {
    let Some(memory) = memory::manager() else {
        return Err(MemoryAccessError::Busy);
    };
    let mut allocator = memory.frame_allocator().lock();
    let mut frames = Vec::with_capacity(page_count);
    for page_index in 0..page_count {
        let Some(frame) = allocator.allocate_4kib() else {
            return Err(MemoryAccessError::Busy);
        };
        let frame_base = frame.base;
        let start = page_index * PAGE_SIZE_BYTES as usize;
        let end = (start + PAGE_SIZE_BYTES as usize).min(bytes.len());
        unsafe {
            core::ptr::write_bytes(frame_base.as_u64() as *mut u8, 0, PAGE_SIZE_BYTES as usize);
            if start < end {
                core::ptr::copy_nonoverlapping(
                    bytes[start..end].as_ptr(),
                    frame_base.as_u64() as *mut u8,
                    end - start,
                );
            }
        }
        frames.push(frame_base);
    }
    Ok(Arc::from(frames.into_boxed_slice()))
}

fn read_page_backed(
    frames: &[PhysicalAddress],
    size_bytes: usize,
    offset: usize,
    destination: &mut [u8],
) -> usize {
    if offset >= size_bytes {
        return 0;
    }
    let mut remaining = destination.len().min(size_bytes - offset);
    let mut destination_offset = 0usize;
    let mut position = offset;
    while remaining > 0 {
        let page_index = position / PAGE_SIZE_BYTES as usize;
        let page_offset = position % PAGE_SIZE_BYTES as usize;
        let copy_len = remaining.min(PAGE_SIZE_BYTES as usize - page_offset);
        let frame_base = frames[page_index].as_u64() as *const u8;
        unsafe {
            core::ptr::copy_nonoverlapping(
                frame_base.add(page_offset),
                destination.as_mut_ptr().add(destination_offset),
                copy_len,
            );
        }
        remaining -= copy_len;
        destination_offset += copy_len;
        position += copy_len;
    }
    destination_offset
}

fn write_page_backed(
    frames: &[PhysicalAddress],
    size_bytes: usize,
    offset: usize,
    source: &[u8],
) -> usize {
    if offset >= size_bytes {
        return 0;
    }
    let mut remaining = source.len().min(size_bytes - offset);
    let mut source_offset = 0usize;
    let mut position = offset;
    while remaining > 0 {
        let page_index = position / PAGE_SIZE_BYTES as usize;
        let page_offset = position % PAGE_SIZE_BYTES as usize;
        let copy_len = remaining.min(PAGE_SIZE_BYTES as usize - page_offset);
        let frame_base = frames[page_index].as_u64() as *mut u8;
        unsafe {
            core::ptr::copy_nonoverlapping(
                source.as_ptr().add(source_offset),
                frame_base.add(page_offset),
                copy_len,
            );
        }
        remaining -= copy_len;
        source_offset += copy_len;
        position += copy_len;
    }
    source_offset
}

pub const PIPE_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PipeSnapshot {
    pub readable_bytes: usize,
    pub free_bytes: usize,
    pub readers: u32,
    pub writers: u32,
}

/// Nonblocking read result. `Bytes(0)` means "no data yet, writers alive";
/// [`PipeReadOutcome::EndOfStream`] means the buffer is drained and every
/// writer handle is closed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PipeReadOutcome {
    Bytes(usize),
    EndOfStream,
}

/// Nonblocking write result. `WouldBlock` means the ring is full while a
/// reader remains; [`PipeWriteOutcome::BrokenPipe`] means every reader
/// handle is closed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PipeWriteOutcome {
    Bytes(usize),
    WouldBlock,
    BrokenPipe,
}

struct PipeState {
    buf: Box<[u8; PIPE_BUFFER_BYTES]>,
    head: usize,
    len: usize,
    readers: u32,
    writers: u32,
}

/// Bounded byte-stream pipe: a 64 KiB ring buffer with explicit reader and
/// writer side refcounts. All primitives are nonblocking; syscall handlers
/// layer block/wakeup on top via the object-wait substrate so this type stays
/// host-testable without a scheduler.
pub struct PipeObject {
    state: Mutex<PipeState>,
}

impl PipeObject {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(PipeState {
                buf: Box::new([0; PIPE_BUFFER_BYTES]),
                head: 0,
                len: 0,
                readers: 1,
                writers: 1,
            }),
        }
    }

    pub fn snapshot(&self) -> PipeSnapshot {
        let state = self.state.lock();
        PipeSnapshot {
            readable_bytes: state.len,
            free_bytes: PIPE_BUFFER_BYTES - state.len,
            readers: state.readers,
            writers: state.writers,
        }
    }

    pub fn read(&self, out: &mut [u8]) -> PipeReadOutcome {
        let mut state = self.state.lock();
        if state.len == 0 {
            return if state.writers == 0 {
                PipeReadOutcome::EndOfStream
            } else {
                PipeReadOutcome::Bytes(0)
            };
        }
        if out.is_empty() {
            return PipeReadOutcome::Bytes(0);
        }
        let tail = (state.head + PIPE_BUFFER_BYTES - state.len) % PIPE_BUFFER_BYTES;
        let contiguous = PIPE_BUFFER_BYTES - tail;
        let count = out.len().min(state.len).min(contiguous);
        out[..count].copy_from_slice(&state.buf[tail..tail + count]);
        state.len -= count;
        PipeReadOutcome::Bytes(count)
    }

    pub fn write(&self, bytes: &[u8]) -> PipeWriteOutcome {
        // Zero-length writes stay benign no-ops even on a broken pipe,
        // matching the usual kernel convention.
        if bytes.is_empty() {
            return PipeWriteOutcome::Bytes(0);
        }
        let mut state = self.state.lock();
        if state.readers == 0 {
            return PipeWriteOutcome::BrokenPipe;
        }
        let space = PIPE_BUFFER_BYTES - state.len;
        if space == 0 {
            return PipeWriteOutcome::WouldBlock;
        }
        let count = space.min(bytes.len());
        let head = state.head;
        let contiguous = PIPE_BUFFER_BYTES - head;
        let first = count.min(contiguous);
        state.buf[head..head + first].copy_from_slice(&bytes[..first]);
        if count > first {
            state.buf[..count - first].copy_from_slice(&bytes[first..count]);
        }
        state.head = (head + count) % PIPE_BUFFER_BYTES;
        state.len += count;
        PipeWriteOutcome::Bytes(count)
    }

    /// Drops one writer reference; returns true when the last writer closed.
    pub fn close_writer(&self) -> bool {
        let mut state = self.state.lock();
        if state.writers > 0 {
            state.writers -= 1;
        }
        state.writers == 0
    }

    /// Drops one reader reference; returns true when the last reader closed.
    pub fn close_reader(&self) -> bool {
        let mut state = self.state.lock();
        if state.readers > 0 {
            state.readers -= 1;
        }
        state.readers == 0
    }

    pub fn add_reader(&self) {
        let mut state = self.state.lock();
        state.readers = state.readers.saturating_add(1);
    }

    pub fn add_writer(&self) {
        let mut state = self.state.lock();
        state.writers = state.writers.saturating_add(1);
    }
}

#[cfg(test)]
mod pipe_tests {
    use super::*;

    fn drain(pipe: &PipeObject, out: &mut Vec<u8>) -> PipeReadOutcome {
        let mut chunk = [0u8; 512];
        loop {
            match pipe.read(&mut chunk) {
                PipeReadOutcome::Bytes(0) => return PipeReadOutcome::Bytes(0),
                PipeReadOutcome::Bytes(n) => out.extend_from_slice(&chunk[..n]),
                other => return other,
            }
        }
    }

    #[test]
    fn round_trip_small_payload() {
        let pipe = PipeObject::new();
        assert_eq!(pipe.write(b"hello"), PipeWriteOutcome::Bytes(5));
        let mut out = [0u8; 8];
        assert_eq!(pipe.read(&mut out), PipeReadOutcome::Bytes(5));
        assert_eq!(&out[..5], b"hello");
    }

    #[test]
    fn ring_wraparound_preserves_stream_order_across_boundary() {
        let pipe = PipeObject::new();

        // Prime the ring so the write head lands near the buffer end.
        let prime = vec![7u8; 60_000];
        assert_eq!(pipe.write(&prime), PipeWriteOutcome::Bytes(60_000));
        let mut drained = Vec::new();
        assert_eq!(drain(&pipe, &mut drained), PipeReadOutcome::Bytes(0));
        assert_eq!(drained, prime);

        // This write must split across the physical end of the ring and
        // come back out in logical order.
        let wrapped: Vec<u8> = (0..10_000).map(|i| (i % 251) as u8 + 1).collect();
        assert_eq!(pipe.write(&wrapped), PipeWriteOutcome::Bytes(10_000));
        let mut got = Vec::new();
        assert_eq!(drain(&pipe, &mut got), PipeReadOutcome::Bytes(0));
        assert_eq!(got, wrapped);

        // Indices remain consistent after full wraparound.
        assert_eq!(pipe.write(b"tail"), PipeWriteOutcome::Bytes(4));
        let mut out = [0u8; 4];
        assert_eq!(pipe.read(&mut out), PipeReadOutcome::Bytes(4));
        assert_eq!(&out, b"tail");
    }

    #[test]
    fn pending_data_still_readable_after_writer_close_then_eof() {
        let pipe = PipeObject::new();
        assert_eq!(pipe.write(b"leftover"), PipeWriteOutcome::Bytes(8));
        assert!(pipe.close_writer());

        let mut out = [0u8; 16];
        assert_eq!(pipe.read(&mut out), PipeReadOutcome::Bytes(8));
        assert_eq!(&out[..8], b"leftover");
        assert_eq!(pipe.read(&mut out), PipeReadOutcome::EndOfStream);
        assert_eq!(pipe.read(&mut out), PipeReadOutcome::EndOfStream);
    }

    #[test]
    fn empty_read_blocks_only_while_writers_remain() {
        let pipe = PipeObject::new();
        let mut out = [0u8; 4];
        assert_eq!(pipe.read(&mut out), PipeReadOutcome::Bytes(0));
        assert!(pipe.close_writer());
        assert_eq!(pipe.read(&mut out), PipeReadOutcome::EndOfStream);
    }

    #[test]
    fn eof_requires_every_writer_to_close() {
        let pipe = PipeObject::new();
        pipe.add_writer();
        let mut out = [0u8; 4];

        assert!(!pipe.close_writer());
        assert_eq!(pipe.read(&mut out), PipeReadOutcome::Bytes(0));

        assert!(pipe.close_writer());
        assert_eq!(pipe.read(&mut out), PipeReadOutcome::EndOfStream);
    }

    #[test]
    fn write_after_last_reader_close_is_broken_pipe() {
        let pipe = PipeObject::new();
        assert!(pipe.close_reader());
        assert_eq!(
            pipe.write(b"nobody listening"),
            PipeWriteOutcome::BrokenPipe
        );

        // Zero-length writes are benign no-ops even when broken.
        assert_eq!(pipe.write(&[]), PipeWriteOutcome::Bytes(0));

        // Buffered data is dropped once both ends are gone; reads report EOF.
        assert!(pipe.close_writer());
        let mut out = [0u8; 4];
        assert_eq!(pipe.read(&mut out), PipeReadOutcome::EndOfStream);
    }

    #[test]
    fn full_ring_reports_would_block_until_reader_drains() {
        let pipe = PipeObject::new();
        let filler = [0xA5u8; 512];
        let mut pushed = 0usize;
        while pushed < PIPE_BUFFER_BYTES {
            match pipe.write(&filler) {
                PipeWriteOutcome::Bytes(n) => pushed += n,
                other => panic!("unexpected write outcome {other:?}"),
            }
        }
        assert_eq!(pipe.snapshot().free_bytes, 0);
        assert_eq!(pipe.write(b"x"), PipeWriteOutcome::WouldBlock);

        let mut out = [0u8; 256];
        assert_eq!(pipe.read(&mut out), PipeReadOutcome::Bytes(256));
        assert_eq!(pipe.write(&[0x5A; 300]), PipeWriteOutcome::Bytes(256));
        assert_eq!(pipe.write(b"x"), PipeWriteOutcome::WouldBlock);
    }

    #[test]
    fn partial_write_fills_exactly_the_free_space() {
        let pipe = PipeObject::new();
        assert_eq!(pipe.write(&[1; 100]), PipeWriteOutcome::Bytes(100));
        assert_eq!(
            pipe.write(&[2; PIPE_BUFFER_BYTES]),
            PipeWriteOutcome::Bytes(PIPE_BUFFER_BYTES - 100)
        );
        assert_eq!(pipe.write(&[3; 1]), PipeWriteOutcome::WouldBlock);
    }

    #[test]
    fn snapshot_reflects_sides_and_counts() {
        let pipe = PipeObject::new();
        pipe.add_reader();
        pipe.add_writer();
        let snap = pipe.snapshot();
        assert_eq!((snap.readers, snap.writers), (2, 2));
        assert!(!pipe.close_reader());
        assert!(pipe.close_reader());
        assert_eq!(pipe.snapshot().readers, 0);
    }
}
