use alloc::sync::Arc;
use spin::Mutex;

use crate::time::MonotonicInstant;

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
    bytes: Option<Arc<[u8]>>,
}

impl MemoryObject {
    pub fn new(size_bytes: usize, writable: bool) -> Self {
        let page_count = size_bytes.div_ceil(4096);

        Self {
            info: MemoryObjectInfo {
                size_bytes,
                page_count,
                writable,
            },
            bytes: None,
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
            bytes: Some(Arc::from(bytes)),
        }
    }

    pub const fn info(&self) -> MemoryObjectInfo {
        self.info
    }

    pub fn read(&self, offset: usize, destination: &mut [u8]) -> usize {
        let Some(bytes) = &self.bytes else {
            return 0;
        };
        let Some(source) = bytes.get(offset..) else {
            return 0;
        };
        let len = source.len().min(destination.len());
        destination[..len].copy_from_slice(&source[..len]);
        len
    }
}
