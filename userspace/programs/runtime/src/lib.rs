#![no_std]

use core::{
    arch::asm,
    fmt::{self, Write},
};

pub use serviceos_abi::{
    ControlTag, Handle, HandlePair, IPC_FLAG_NONBLOCK, IPC_MAX_HANDLES, IPC_MAX_WORDS,
    INVALID_HANDLE, LifecycleEvent, RawMessage, ServiceId, ServiceImageId, SyscallErrorCode,
    SyscallNumber, TaskStateCode, TaskStatus,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Unsupported,
    InvalidCall,
    PermissionDenied,
    NotInitialized,
    InvalidArgument,
    BufferTooSmall,
    QueueEmpty,
    NotFound,
    Busy,
    CapacityExceeded,
    Unknown(u64),
}

impl Error {
    fn from_code(code: u64) -> Self {
        match code {
            x if x == SyscallErrorCode::Unsupported as u64 => Self::Unsupported,
            x if x == SyscallErrorCode::InvalidCall as u64 => Self::InvalidCall,
            x if x == SyscallErrorCode::PermissionDenied as u64 => Self::PermissionDenied,
            x if x == SyscallErrorCode::NotInitialized as u64 => Self::NotInitialized,
            x if x == SyscallErrorCode::InvalidArgument as u64 => Self::InvalidArgument,
            x if x == SyscallErrorCode::BufferTooSmall as u64 => Self::BufferTooSmall,
            x if x == SyscallErrorCode::QueueEmpty as u64 => Self::QueueEmpty,
            x if x == SyscallErrorCode::NotFound as u64 => Self::NotFound,
            x if x == SyscallErrorCode::Busy as u64 => Self::Busy,
            x if x == SyscallErrorCode::CapacityExceeded as u64 => Self::CapacityExceeded,
            other => Self::Unknown(other),
        }
    }
}

pub type Result<T> = core::result::Result<T, Error>;

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

pub fn channel_create() -> Result<HandlePair> {
    let mut pair = HandlePair {
        first: INVALID_HANDLE,
        second: INVALID_HANDLE,
    };
    syscall1(SyscallNumber::ChannelCreate, &mut pair as *mut HandlePair as u64)?;
    Ok(pair)
}

pub fn channel_send(endpoint: Handle, message: &RawMessage) -> Result<()> {
    syscall2(
        SyscallNumber::ChannelSend,
        endpoint as u64,
        message as *const RawMessage as u64,
    )
    .map(|_| ())
}

pub fn channel_receive(endpoint: Handle, message: &mut RawMessage) -> Result<()> {
    syscall2(
        SyscallNumber::ChannelReceive,
        endpoint as u64,
        message as *mut RawMessage as u64,
    )
    .map(|_| ())
}

pub fn channel_receive_nonblocking(endpoint: Handle, message: &mut RawMessage) -> Result<()> {
    message.flags = IPC_FLAG_NONBLOCK;
    let result = channel_receive(endpoint, message);
    message.flags = 0;
    result
}

pub fn channel_receive_blocking(endpoint: Handle, message: &mut RawMessage) -> Result<()> {
    loop {
        match channel_receive_nonblocking(endpoint, message) {
            Ok(()) => return Ok(()),
            Err(Error::QueueEmpty) => yield_current()?,
            Err(error) => return Err(error),
        }
    }
}

pub fn handle_duplicate(handle: Handle, rights: u64) -> Result<Handle> {
    syscall2(
        SyscallNumber::HandleDuplicate,
        handle as u64,
        rights,
    )
    .map(|value| value as Handle)
}

pub fn handle_close(handle: Handle) -> Result<()> {
    syscall1(SyscallNumber::HandleClose, handle as u64).map(|_| ())
}

pub fn service_spawn(image_id: ServiceImageId, bootstrap_handle: Handle) -> Result<Handle> {
    syscall2(
        SyscallNumber::ServiceSpawn,
        image_id as u32 as u64,
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

pub fn write_log(domain: &str, message: &str) -> Result<()> {
    let mut buffer = FixedLogBuffer::<192>::new();
    let _ = write!(&mut buffer, "{domain}: {message}");
    debug_log(buffer.as_bytes())
}

pub fn write_logf(domain: &str, args: fmt::Arguments<'_>) -> Result<()> {
    let mut buffer = FixedLogBuffer::<192>::new();
    let _ = write!(&mut buffer, "{domain}: ");
    let _ = buffer.write_fmt(args);
    debug_log(buffer.as_bytes())
}

pub struct FixedLogBuffer<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> FixedLogBuffer<N> {
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

impl<const N: usize> Write for FixedLogBuffer<N> {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let bytes = value.as_bytes();
        let remaining = N.saturating_sub(self.len);
        let copy_len = remaining.min(bytes.len());
        self.bytes[self.len..self.len + copy_len].copy_from_slice(&bytes[..copy_len]);
        self.len += copy_len;
        Ok(())
    }
}

#[macro_export]
macro_rules! entry {
    ($path:path) => {
        #[panic_handler]
        fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
            let _ = $crate::write_log("panic", "userspace panic");
            $crate::thread_exit(0xffff_ffff_ffff_ff00)
        }

        #[unsafe(no_mangle)]
        #[unsafe(link_section = ".text.start")]
        pub extern "C" fn _start() -> ! {
            let code: u64 = $path();
            $crate::thread_exit(code)
        }
    };
}

fn syscall0(number: SyscallNumber) -> Result<u64> {
    let (value, error) = raw_syscall(number as u32 as u64, 0, 0, 0, 0, 0, 0);
    decode_result(value, error)
}

fn syscall1(number: SyscallNumber, arg0: u64) -> Result<u64> {
    let (value, error) = raw_syscall(number as u32 as u64, arg0, 0, 0, 0, 0, 0);
    decode_result(value, error)
}

fn syscall2(number: SyscallNumber, arg0: u64, arg1: u64) -> Result<u64> {
    let (value, error) = raw_syscall(number as u32 as u64, arg0, arg1, 0, 0, 0, 0);
    decode_result(value, error)
}

fn decode_result(value: u64, error: u64) -> Result<u64> {
    if error == 0 {
        Ok(value)
    } else {
        Err(Error::from_code(error))
    }
}

fn raw_syscall(
    number: u64,
    arg0: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    arg4: u64,
    arg5: u64,
) -> (u64, u64) {
    let mut value = number;
    let mut arg2_inout = arg2;

    unsafe {
        asm!(
            "int 0x80",
            inlateout("rax") value,
            in("rdi") arg0,
            in("rsi") arg1,
            inlateout("rdx") arg2_inout,
            in("r10") arg3,
            in("r8") arg4,
            in("r9") arg5,
        );
    }

    (value, arg2_inout)
}
