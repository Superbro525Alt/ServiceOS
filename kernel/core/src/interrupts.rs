use core::sync::atomic::{AtomicU64, Ordering};

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

pub fn initialize() -> &'static InterruptState {
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
    time::manager().map(|manager| manager.handle_tick())
}

pub fn handle_exception(detail: ExceptionDetail, frame: TrapFrameView) -> ExceptionReport {
    if let Some(state) = state() {
        state.exceptions.fetch_add(1, Ordering::Relaxed);
    }

    let disposition = classify_exception(detail, frame.origin());

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
