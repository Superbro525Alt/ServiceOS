use core::{ptr::NonNull, slice};

use crate::{syscall2, syscall4, Handle, Result, SyscallNumber};

pub struct MappedMemory {
    ptr: NonNull<u8>,
    len: usize,
}

impl MappedMemory {
    pub fn map(handle: Handle, len: usize, writable: bool) -> Result<Self> {
        let ptr = memory_map(handle, writable)?;
        let ptr = NonNull::new(ptr).ok_or(crate::Error::Busy)?;
        Ok(Self { ptr, len })
    }

    pub fn as_slice(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self.ptr.as_ptr() as *const u8, self.len) }
    }

    pub fn as_slice_mut(&mut self) -> &mut [u8] {
        unsafe { slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }

    pub fn as_ptr(&self) -> *mut u8 {
        self.ptr.as_ptr()
    }
}

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

pub fn memory_map(handle: Handle, writable: bool) -> Result<*mut u8> {
    syscall2(
        SyscallNumber::MemoryMap,
        handle as u64,
        u64::from(writable),
    )
    .map(|value| value as *mut u8)
}
