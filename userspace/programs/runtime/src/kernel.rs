use crate::{
    Handle, HandlePair, INVALID_HANDLE, KernelEventRecord, OBJECT_WAIT_FLAG_NONBLOCK, ObjectInfo,
    PIPE_FLAG_NONBLOCK, Result, ServiceImageId, SyscallNumber, TaskStateCode, TaskStatus, syscall0,
    syscall1, syscall2, syscall3, syscall4,
};

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

pub fn pipe_create() -> Result<(Handle, Handle)> {
    let mut pair = HandlePair {
        first: INVALID_HANDLE,
        second: INVALID_HANDLE,
    };
    syscall1(
        SyscallNumber::PipeCreate,
        &mut pair as *mut HandlePair as u64,
    )?;
    Ok((pair.first, pair.second))
}

pub fn pipe_read(handle: Handle, buffer: &mut [u8], nonblock: bool) -> Result<usize> {
    let count = syscall4(
        SyscallNumber::PipeRead,
        handle as u64,
        buffer.as_mut_ptr() as u64,
        buffer.len() as u64,
        if nonblock {
            PIPE_FLAG_NONBLOCK as u64
        } else {
            0
        },
    )?;
    Ok(count as usize)
}

pub fn pipe_write(handle: Handle, bytes: &[u8], nonblock: bool) -> Result<usize> {
    let count = syscall4(
        SyscallNumber::PipeWrite,
        handle as u64,
        bytes.as_ptr() as u64,
        bytes.len() as u64,
        if nonblock {
            PIPE_FLAG_NONBLOCK as u64
        } else {
            0
        },
    )?;
    Ok(count as usize)
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

pub fn task_spawn_image(
    image_handle: Handle,
    bootstrap_authority: Handle,
    bootstrap_handle: Handle,
) -> Result<Handle> {
    syscall3(
        SyscallNumber::TaskSpawnImage,
        image_handle as u64,
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

pub fn object_info(handle: Handle) -> Result<ObjectInfo> {
    let mut info = ObjectInfo {
        object_id: 0,
        kind: crate::ObjectKindCode::Task,
        state_flags: 0,
        reserved: 0,
        detail0: 0,
        detail1: 0,
        detail2: 0,
        detail3: 0,
    };
    syscall2(
        SyscallNumber::ObjectInfo,
        handle as u64,
        &mut info as *mut ObjectInfo as u64,
    )?;
    Ok(info)
}

pub fn object_wait(handle: Handle, nonblock: bool) -> Result<()> {
    syscall2(
        SyscallNumber::ObjectWait,
        handle as u64,
        if nonblock {
            OBJECT_WAIT_FLAG_NONBLOCK as u64
        } else {
            0
        },
    )
    .map(|_| ())
}

pub fn event_create(signaled: bool) -> Result<Handle> {
    syscall1(SyscallNumber::EventCreate, u64::from(signaled)).map(|value| value as Handle)
}

pub fn event_signal(handle: Handle) -> Result<()> {
    syscall1(SyscallNumber::EventSignal, handle as u64).map(|_| ())
}

pub fn event_reset(handle: Handle) -> Result<()> {
    syscall1(SyscallNumber::EventReset, handle as u64).map(|_| ())
}

pub fn kernel_event_query_info() -> Result<(u64, u64)> {
    let packed = syscall0(SyscallNumber::KernelEventQueryInfo)?;
    Ok((packed & 0xffff_ffff, packed >> 32))
}

pub fn kernel_event_query_record(sequence: u64) -> Result<Option<KernelEventRecord>> {
    let mut record = KernelEventRecord {
        sequence: 0,
        kind: crate::KernelEventKind::Trap,
        reserved: 0,
        tick: 0,
        detail0: 0,
        detail1: 0,
        detail2: 0,
        detail3: 0,
        detail4: 0,
    };
    match syscall2(
        SyscallNumber::KernelEventQueryRecord,
        sequence,
        &mut record as *mut KernelEventRecord as u64,
    ) {
        Ok(_) => Ok(Some(record)),
        Err(crate::Error::NotFound) => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn wait_for_exit(task_handle: Handle) -> Result<TaskStatus> {
    loop {
        object_wait(task_handle, false)?;
        let status = task_status(task_handle)?;
        if !matches!(status.state, TaskStateCode::Running) {
            return Ok(status);
        }
    }
}
