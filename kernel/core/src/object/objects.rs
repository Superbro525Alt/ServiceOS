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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryObjectInfo {
    pub size_bytes: usize,
    pub page_count: usize,
    pub writable: bool,
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
}

impl MemoryObject {
    pub fn new(size_bytes: usize, writable: bool) -> Self {
        let page_count = size_bytes.div_ceil(4096);
        let zeroed = vec![0u8; size_bytes].into_boxed_slice();

        Self {
            info: MemoryObjectInfo {
                size_bytes,
                page_count,
                writable,
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
        match &state.backing {
            WritableMemoryBacking::PageBacked(frames) => Ok(Arc::clone(frames)),
            WritableMemoryBacking::Linear(bytes) => {
                let frames = allocate_page_backing(bytes, self.info.page_count)?;
                state.backing = WritableMemoryBacking::PageBacked(Arc::clone(&frames));
                Ok(frames)
            }
        }
    }
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
