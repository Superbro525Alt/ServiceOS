use core::{
    arch::asm,
    fmt::{self, Write},
};

use crate::{debug_log, Error, Result, SyscallNumber};

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

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(self.as_bytes()).unwrap_or("")
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

pub(crate) fn syscall0(number: SyscallNumber) -> Result<u64> {
    let (value, error) = raw_syscall(number as u32 as u64, 0, 0, 0, 0, 0, 0);
    decode_result(value, error)
}

pub(crate) fn syscall1(number: SyscallNumber, arg0: u64) -> Result<u64> {
    let (value, error) = raw_syscall(number as u32 as u64, arg0, 0, 0, 0, 0, 0);
    decode_result(value, error)
}

pub(crate) fn syscall2(number: SyscallNumber, arg0: u64, arg1: u64) -> Result<u64> {
    let (value, error) = raw_syscall(number as u32 as u64, arg0, arg1, 0, 0, 0, 0);
    decode_result(value, error)
}

pub(crate) fn syscall3(number: SyscallNumber, arg0: u64, arg1: u64, arg2: u64) -> Result<u64> {
    let (value, error) = raw_syscall(number as u32 as u64, arg0, arg1, arg2, 0, 0, 0);
    decode_result(value, error)
}

pub(crate) fn syscall4(
    number: SyscallNumber,
    arg0: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
) -> Result<u64> {
    let (value, error) = raw_syscall(number as u32 as u64, arg0, arg1, arg2, arg3, 0, 0);
    decode_result(value, error)
}

pub(crate) fn syscall5(
    number: SyscallNumber,
    arg0: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    arg4: u64,
) -> Result<u64> {
    let (value, error) = raw_syscall(number as u32 as u64, arg0, arg1, arg2, arg3, arg4, 0);
    decode_result(value, error)
}

pub fn pack_bytes(source: &[u8], words: &mut [u64]) -> Result<u32> {
    let required_words = source.len().div_ceil(8);
    if required_words > words.len() {
        return Err(Error::BufferTooSmall);
    }
    for (index, chunk) in source.chunks(8).enumerate() {
        let mut bytes = [0u8; 8];
        bytes[..chunk.len()].copy_from_slice(chunk);
        words[index] = u64::from_le_bytes(bytes);
    }
    Ok(required_words as u32)
}

pub fn unpack_bytes(words: &[u64], len: usize, destination: &mut [u8]) -> Result<()> {
    if len > destination.len() || len > words.len() * 8 {
        return Err(Error::BufferTooSmall);
    }

    let mut copied = 0usize;
    for word in words.iter().copied() {
        if copied >= len {
            break;
        }
        let bytes = word.to_le_bytes();
        let chunk = (len - copied).min(bytes.len());
        destination[copied..copied + chunk].copy_from_slice(&bytes[..chunk]);
        copied += chunk;
    }
    Ok(())
}

fn decode_result(value: u64, error: u64) -> Result<u64> {
    if error == 0 {
        Ok(value)
    } else {
        Err(Error::from_code(error))
    }
}

#[cfg(target_arch = "x86_64")]
fn raw_syscall(
    number: u64,
    arg0: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    arg4: u64,
    arg5: u64,
) -> (u64, u64) {
    let value: u64;
    let error: u64;
    unsafe {
        asm!(
            "int 0x80",
            inlateout("rax") number => value,
            in("rdi") arg0,
            in("rsi") arg1,
            inlateout("rdx") arg2 => error,
            in("r10") arg3,
            in("r8") arg4,
            in("r9") arg5,
        );
    }
    (value, error)
}

#[cfg(target_arch = "aarch64")]
fn raw_syscall(
    number: u64,
    arg0: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    arg4: u64,
    arg5: u64,
) -> (u64, u64) {
    let mut value = arg0;
    let mut error = arg1;
    unsafe {
        asm!(
            "svc #0",
            in("x8") number,
            inlateout("x0") value,
            inlateout("x1") error,
            in("x2") arg2,
            in("x3") arg3,
            in("x4") arg4,
            in("x5") arg5,
            options(nostack),
        );
    }
    (value, error)
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
fn raw_syscall(
    _number: u64,
    _arg0: u64,
    _arg1: u64,
    _arg2: u64,
    _arg3: u64,
    _arg4: u64,
    _arg5: u64,
) -> (u64, u64) {
    (0, SyscallErrorCode::Unsupported as u64)
}
