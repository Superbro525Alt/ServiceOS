use crate::{syscall2, syscall4, Handle, Result, SyscallNumber};

pub fn memory_read(handle: Handle, offset: usize, buffer: &mut [u8]) -> Result<usize> {
    syscall4(
        SyscallNumber::MemoryRead,
        handle as u64,
        offset as u64,
        buffer.as_mut_ptr() as u64,
        buffer.len() as u64,
    )
    .map(|value| value as usize)
}

pub fn memory_create(size_bytes: usize, writable: bool) -> Result<Handle> {
    syscall2(
        SyscallNumber::MemoryCreate,
        size_bytes as u64,
        u64::from(writable),
    )
    .map(|value| value as Handle)
}

pub fn memory_write(handle: Handle, offset: usize, bytes: &[u8]) -> Result<usize> {
    syscall4(
        SyscallNumber::MemoryWrite,
        handle as u64,
        offset as u64,
        bytes.as_ptr() as u64,
        bytes.len() as u64,
    )
    .map(|value| value as usize)
}
