use core::sync::atomic::{AtomicU64, Ordering};

use spin::Once;

use crate::time;

const SYSCALL_ABI_VERSION: u64 = 0x0002_0000;
const MAX_SYSCALL_SLOTS: usize = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyscallNumber(pub u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyscallContext {
    pub instruction_pointer: u64,
    pub stack_pointer: u64,
    pub flags: u64,
    pub arguments: [u64; 6],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyscallReturn {
    pub value: u64,
    pub error: Option<SyscallError>,
    pub action: SyscallAction,
}

impl SyscallReturn {
    pub const fn success(value: u64) -> Self {
        Self {
            value,
            error: None,
            action: SyscallAction::ReturnToCaller,
        }
    }

    pub const fn error(error: SyscallError) -> Self {
        Self {
            value: 0,
            error: Some(error),
            action: SyscallAction::ReturnToCaller,
        }
    }

    pub const fn exit_current_thread(status: u64) -> Self {
        Self {
            value: status,
            error: None,
            action: SyscallAction::ExitCurrentThread { status },
        }
    }

    pub const fn abi_error_code(self) -> u64 {
        match self.error {
            None => 0,
            Some(error) => error.abi_code(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyscallAction {
    ReturnToCaller,
    ExitCurrentThread { status: u64 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyscallError {
    UnsupportedInPhase2,
    InvalidCall,
    PermissionDenied,
    NotInitialized,
}

impl SyscallError {
    pub const fn abi_code(self) -> u64 {
        match self {
            Self::UnsupportedInPhase2 => 1,
            Self::InvalidCall => 2,
            Self::PermissionDenied => 3,
            Self::NotInitialized => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyscallKind {
    AbiVersion = 0,
    MonotonicNow = 1,
    ThreadExit = 2,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyscallSnapshot {
    pub dispatched: u64,
    pub rejected: u64,
}

pub trait SyscallDispatcher {
    fn dispatch(&self, number: SyscallNumber, context: &SyscallContext) -> SyscallReturn;
}

type Handler = fn(&SyscallContext) -> SyscallReturn;

pub struct DispatchTable {
    entries: [Option<Handler>; MAX_SYSCALL_SLOTS],
    dispatched: AtomicU64,
    rejected: AtomicU64,
}

impl DispatchTable {
    const fn new(entries: [Option<Handler>; MAX_SYSCALL_SLOTS]) -> Self {
        Self {
            entries,
            dispatched: AtomicU64::new(0),
            rejected: AtomicU64::new(0),
        }
    }

    pub fn snapshot(&self) -> SyscallSnapshot {
        SyscallSnapshot {
            dispatched: self.dispatched.load(Ordering::Relaxed),
            rejected: self.rejected.load(Ordering::Relaxed),
        }
    }
}

impl SyscallDispatcher for DispatchTable {
    fn dispatch(&self, number: SyscallNumber, context: &SyscallContext) -> SyscallReturn {
        self.dispatched.fetch_add(1, Ordering::Relaxed);

        let handler = self
            .entries
            .get(number.0 as usize)
            .and_then(|entry| entry.as_ref().copied());

        match handler {
            Some(handler) => handler(context),
            None => {
                self.rejected.fetch_add(1, Ordering::Relaxed);
                SyscallReturn::error(SyscallError::InvalidCall)
            }
        }
    }
}

static DISPATCHER: Once<DispatchTable> = Once::new();

pub fn initialize() -> &'static DispatchTable {
    DISPATCHER.call_once(|| {
        DispatchTable::new([
            Some(handle_abi_version),
            Some(handle_monotonic_now),
            Some(handle_thread_exit),
        ])
    })
}

pub fn dispatcher() -> Option<&'static DispatchTable> {
    DISPATCHER.get()
}

fn handle_abi_version(_context: &SyscallContext) -> SyscallReturn {
    SyscallReturn::success(SYSCALL_ABI_VERSION)
}

fn handle_monotonic_now(_context: &SyscallContext) -> SyscallReturn {
    match time::manager() {
        Some(manager) => SyscallReturn::success(manager.now().0),
        None => SyscallReturn::error(SyscallError::NotInitialized),
    }
}

fn handle_thread_exit(context: &SyscallContext) -> SyscallReturn {
    SyscallReturn::exit_current_thread(context.arguments[0])
}
