use core::sync::atomic::{AtomicU64, Ordering};

use serviceos_abi::{KernelEventKind, KernelEventRecord};
use spin::Mutex;
use spin::Once;

use crate::{
    syscall::{self, SyscallContext, SyscallDispatcher, SyscallNumber, SyscallReturn},
    time::{self, MonotonicInstant, TickOutcome},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct InterruptVector(pub u16);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct ExceptionVector(pub u8);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TrapFrameView {
    pub instruction_pointer: u64,
    pub stack_pointer: u64,
    pub flags: u64,
    pub code_segment: u64,
}

impl TrapFrameView {
    pub const fn origin(self) -> TrapOrigin {
        if self.code_segment & 0b11 == 0b11 {
            TrapOrigin::User
        } else {
            TrapOrigin::Kernel
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrapOrigin {
    Kernel,
    User,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FaultDisposition {
    Fatal,
    Retry,
    TerminateTask,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExceptionDetail {
    Breakpoint,
    InvalidOpcode,
    PageFault {
        fault_address: u64,
        error_code: u64,
    },
    GeneralProtection {
        error_code: u64,
    },
    DoubleFault {
        error_code: u64,
    },
    Unknown {
        vector: ExceptionVector,
        error_code: Option<u64>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExceptionReport {
    pub detail: ExceptionDetail,
    pub frame: TrapFrameView,
    pub disposition: FaultDisposition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InterruptSnapshot {
    pub external_interrupts: u64,
    pub exceptions: u64,
    pub timer_interrupts: u64,
    pub syscalls: u64,
}

pub trait InterruptController {
    fn enable_vector(&mut self, vector: InterruptVector);
    fn disable_vector(&mut self, vector: InterruptVector);
}

pub struct InterruptState {
    external_interrupts: AtomicU64,
    exceptions: AtomicU64,
    timer_interrupts: AtomicU64,
    syscalls: AtomicU64,
}

impl InterruptState {
    const fn new() -> Self {
        Self {
            external_interrupts: AtomicU64::new(0),
            exceptions: AtomicU64::new(0),
            timer_interrupts: AtomicU64::new(0),
            syscalls: AtomicU64::new(0),
        }
    }

    pub fn snapshot(&self) -> InterruptSnapshot {
        InterruptSnapshot {
            external_interrupts: self.external_interrupts.load(Ordering::Relaxed),
            exceptions: self.exceptions.load(Ordering::Relaxed),
            timer_interrupts: self.timer_interrupts.load(Ordering::Relaxed),
            syscalls: self.syscalls.load(Ordering::Relaxed),
        }
    }
}

static INTERRUPT_STATE: Once<InterruptState> = Once::new();
static KERNEL_EVENTS: Once<Mutex<KernelEventBuffer>> = Once::new();

pub fn initialize() -> &'static InterruptState {
    let _ = KERNEL_EVENTS.call_once(|| Mutex::new(KernelEventBuffer::new()));
    INTERRUPT_STATE.call_once(InterruptState::new)
}

pub fn state() -> Option<&'static InterruptState> {
    INTERRUPT_STATE.get()
}

pub fn note_external_interrupt(_vector: InterruptVector) {
    if let Some(state) = state() {
        state.external_interrupts.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn note_timer_interrupt(_vector: InterruptVector) -> Option<TickOutcome> {
    let state = state()?;
    state.timer_interrupts.fetch_add(1, Ordering::Relaxed);
    if let Some(scheduler) = crate::task::system().map(|s| s.scheduler()) {
        scheduler.handle_tick();
    }
    time::manager().map(|manager| manager.handle_tick())
}

pub fn handle_exception(detail: ExceptionDetail, frame: TrapFrameView) -> ExceptionReport {
    if let Some(state) = state() {
        state.exceptions.fetch_add(1, Ordering::Relaxed);
    }

    let disposition = classify_exception(detail, frame.origin());
    note_kernel_event(kernel_event_for_exception(detail, frame, disposition));

    ExceptionReport {
        detail,
        frame,
        disposition,
    }
}

pub fn dispatch_syscall(number: SyscallNumber, context: &SyscallContext) -> SyscallReturn {
    if let Some(state) = state() {
        state.syscalls.fetch_add(1, Ordering::Relaxed);
    }

    syscall::dispatcher()
        .map(|dispatcher| dispatcher.dispatch(number, context))
        .unwrap_or_else(|| SyscallReturn::error(syscall::SyscallError::NotInitialized))
}

pub fn monotonic_now() -> Option<MonotonicInstant> {
    time::manager().map(|manager| manager.now())
}

fn classify_exception(detail: ExceptionDetail, origin: TrapOrigin) -> FaultDisposition {
    match detail {
        ExceptionDetail::Breakpoint => FaultDisposition::Retry,
        ExceptionDetail::DoubleFault { .. } => FaultDisposition::Fatal,
        ExceptionDetail::InvalidOpcode
        | ExceptionDetail::PageFault { .. }
        | ExceptionDetail::GeneralProtection { .. }
        | ExceptionDetail::Unknown { .. } => match origin {
            TrapOrigin::Kernel => FaultDisposition::Fatal,
            TrapOrigin::User => FaultDisposition::TerminateTask,
        },
    }
}

pub fn kernel_event_info() -> (u64, u64) {
    KERNEL_EVENTS
        .get()
        .map(|buffer| buffer.lock().info())
        .unwrap_or((0, 0))
}

pub fn kernel_event_query(sequence: u64) -> Option<KernelEventRecord> {
    KERNEL_EVENTS
        .get()
        .and_then(|buffer| buffer.lock().query(sequence))
}

const MAX_KERNEL_EVENTS: usize = 64;

struct KernelEventBuffer {
    records: [KernelEventRecord; MAX_KERNEL_EVENTS],
    next_slot: usize,
    count: usize,
    next_sequence: u64,
}

impl KernelEventBuffer {
    const fn new() -> Self {
        Self {
            records: [KernelEventRecord {
                sequence: 0,
                kind: KernelEventKind::Trap,
                reserved: 0,
                tick: 0,
                detail0: 0,
                detail1: 0,
                detail2: 0,
                detail3: 0,
                detail4: 0,
            }; MAX_KERNEL_EVENTS],
            next_slot: 0,
            count: 0,
            next_sequence: 1,
        }
    }

    fn push(&mut self, mut record: KernelEventRecord) {
        record.sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.records[self.next_slot] = record;
        self.next_slot = (self.next_slot + 1) % self.records.len();
        self.count = (self.count + 1).min(self.records.len());
    }

    fn info(&self) -> (u64, u64) {
        if self.count == 0 {
            (0, 0)
        } else {
            (
                self.next_sequence
                    .saturating_sub(self.count as u64)
                    .saturating_add(0),
                self.next_sequence,
            )
        }
    }

    fn query(&self, sequence: u64) -> Option<KernelEventRecord> {
        self.records[..self.count]
            .iter()
            .copied()
            .find(|record| record.sequence == sequence)
    }
}

fn note_kernel_event(record: KernelEventRecord) {
    if let Some(buffer) = KERNEL_EVENTS.get() {
        buffer.lock().push(record);
    }
}

fn kernel_event_for_exception(
    detail: ExceptionDetail,
    frame: TrapFrameView,
    disposition: FaultDisposition,
) -> KernelEventRecord {
    let (detail0, detail3, detail4) = match detail {
        ExceptionDetail::Breakpoint => (3, 0, 0),
        ExceptionDetail::InvalidOpcode => (6, 0, 0),
        ExceptionDetail::PageFault {
            fault_address,
            error_code,
        } => (14, fault_address, error_code),
        ExceptionDetail::GeneralProtection { error_code } => (13, 0, error_code),
        ExceptionDetail::DoubleFault { error_code } => (8, 0, error_code),
        ExceptionDetail::Unknown { vector, error_code } => {
            (vector.0 as u64, vector.0 as u64, error_code.unwrap_or(0))
        }
    };
    KernelEventRecord {
        sequence: 0,
        kind: KernelEventKind::Trap,
        reserved: 0,
        tick: monotonic_now().map_or(0, |instant| instant.0),
        detail0,
        detail1: match frame.origin() {
            TrapOrigin::Kernel => 0,
            TrapOrigin::User => 1,
        },
        detail2: frame.instruction_pointer,
        detail3,
        detail4: ((frame.stack_pointer & 0xffff_ffff) << 32)
            | ((disposition as u64) & 0xffff)
            | ((detail4 & 0xffff) << 16),
    }
}
