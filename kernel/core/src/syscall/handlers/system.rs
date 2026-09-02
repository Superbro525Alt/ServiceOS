use serviceos_abi::KernelEventRecord as AbiKernelEventRecord;

use super::super::SYSCALL_ABI_VERSION;

use super::{
    super::{
        SyscallAction, SyscallContext, SyscallError, SyscallReturn, user_mut, user_slice,
        user_slice_mut,
    },
    common::{DEBUG_CONSOLE_READER, DEBUG_CONSOLE_WRITER, DEBUG_LOG_WRITER},
};
use crate::{interrupts, time};

pub(crate) fn handle_abi_version(_context: &SyscallContext) -> SyscallReturn {
    SyscallReturn::success(SYSCALL_ABI_VERSION)
}

pub(crate) fn handle_monotonic_now(_context: &SyscallContext) -> SyscallReturn {
    match time::manager() {
        Some(manager) => SyscallReturn::success(manager.now().0),
        None => SyscallReturn::error(SyscallError::NotInitialized),
    }
}

pub(crate) fn handle_thread_exit(context: &SyscallContext) -> SyscallReturn {
    SyscallReturn::exit_current_thread(context.arguments[0])
}

pub(crate) fn handle_yield_current(_context: &SyscallContext) -> SyscallReturn {
    SyscallReturn::action(0, SyscallAction::YieldCurrentThread)
}

pub(crate) fn handle_debug_log_write(context: &SyscallContext) -> SyscallReturn {
    let Some(writer) = DEBUG_LOG_WRITER.get().copied() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Ok(length) = usize::try_from(context.arguments[1]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(bytes) = (unsafe { user_slice(context.arguments[0], length) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    writer(bytes);
    SyscallReturn::success(length as u64)
}

pub(crate) fn handle_debug_console_read(_context: &SyscallContext) -> SyscallReturn {
    let Some(reader) = DEBUG_CONSOLE_READER.get().copied() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };

    match reader() {
        Some(byte) => SyscallReturn::success(byte as u64),
        None => SyscallReturn::error(SyscallError::QueueEmpty),
    }
}

pub(crate) fn handle_debug_console_write(context: &SyscallContext) -> SyscallReturn {
    let Some(writer) = DEBUG_CONSOLE_WRITER.get().copied() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Ok(length) = usize::try_from(context.arguments[1]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(bytes) = (unsafe { user_slice(context.arguments[0], length) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    writer(bytes);
    SyscallReturn::success(length as u64)
}

pub(crate) fn handle_kernel_event_query_info(_context: &SyscallContext) -> SyscallReturn {
    let (oldest, next) = interrupts::kernel_event_info();
    let value = (next << 32) | (oldest & 0xffff_ffff);
    SyscallReturn::success(value)
}

/// RngRequest: fill the caller's buffer from the kernel DRBG. Arguments:
/// (buffer pointer, max length). Returns the number of bytes written;
/// NotInitialized when the kernel RNG was never seeded (platforms without
/// any seed path), so callers keep their documented substitutes.
pub(crate) fn handle_rng_request(context: &SyscallContext) -> SyscallReturn {
    let Ok(length) = usize::try_from(context.arguments[1]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    if length > crate::rng::MAX_REQUEST_BYTES {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    }
    let Ok(bytes) = (unsafe { user_slice_mut(context.arguments[0], length) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    if !crate::rng::fill(bytes) {
        return SyscallReturn::error(SyscallError::NotInitialized);
    }
    SyscallReturn::success(length as u64)
}

pub(crate) fn handle_kernel_event_query_record(context: &SyscallContext) -> SyscallReturn {
    let Ok(record_out) = (unsafe { user_mut::<AbiKernelEventRecord>(context.arguments[1]) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Some(record) = interrupts::kernel_event_query(context.arguments[0]) else {
        return SyscallReturn::error(SyscallError::NotFound);
    };
    *record_out = record;
    SyscallReturn::success(0)
}
