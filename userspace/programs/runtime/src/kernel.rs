use crate::{syscall0, syscall1, syscall2, syscall3, Handle, Result, ServiceImageId, SyscallNumber, TaskStateCode, TaskStatus};

pub fn abi_version() -> Result<u64> {
    syscall0(SyscallNumber::AbiVersion)
}

pub fn monotonic_now() -> Result<u64> {
    syscall0(SyscallNumber::MonotonicNow)
}

pub fn yield_current() -> Result<()> {
    syscall0(SyscallNumber::YieldCurrent).map(|_| ())
}

pub fn thread_exit(code: u64) -> ! {
    let _ = syscall1(SyscallNumber::ThreadExit, code);
    loop {
        core::hint::spin_loop();
    }
}

pub fn debug_log(message: &[u8]) -> Result<()> {
    syscall2(
        SyscallNumber::DebugLogWrite,
        message.as_ptr() as u64,
        message.len() as u64,
    )
    .map(|_| ())
}

pub fn debug_console_read_byte() -> Result<u8> {
    syscall0(SyscallNumber::DebugConsoleRead).map(|value| value as u8)
}

pub fn debug_console_write(bytes: &[u8]) -> Result<()> {
    syscall2(
        SyscallNumber::DebugConsoleWrite,
        bytes.as_ptr() as u64,
        bytes.len() as u64,
    )
    .map(|_| ())
}

pub fn handle_duplicate(handle: Handle, rights: u64) -> Result<Handle> {
    syscall2(SyscallNumber::HandleDuplicate, handle as u64, rights).map(|value| value as Handle)
}

pub fn handle_close(handle: Handle) -> Result<()> {
    syscall1(SyscallNumber::HandleClose, handle as u64).map(|_| ())
}

pub fn service_spawn(
    image_id: ServiceImageId,
    bootstrap_authority: Handle,
    bootstrap_handle: Handle,
) -> Result<Handle> {
    syscall3(
        SyscallNumber::ServiceSpawn,
        image_id as u32 as u64,
        bootstrap_authority as u64,
        bootstrap_handle as u64,
    )
    .map(|value| value as Handle)
}

pub fn task_status(task_handle: Handle) -> Result<TaskStatus> {
    let mut status = TaskStatus {
        state: TaskStateCode::Running,
        exit_code: 0,
    };
    syscall2(
        SyscallNumber::TaskStatus,
        task_handle as u64,
        &mut status as *mut TaskStatus as u64,
    )?;
    Ok(status)
}

pub fn wait_for_exit(task_handle: Handle) -> Result<TaskStatus> {
    loop {
        let status = task_status(task_handle)?;
        if status.state == TaskStateCode::Exited {
            return Ok(status);
        }
        yield_current()?;
    }
}
