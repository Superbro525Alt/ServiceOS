//! Kernel entropy contract: draw DRBG bytes from the kernel's RNG
//! subsystem via the RngRequest syscall (55). Returns Err(NotInitialized)
//! on platforms where the kernel never seeded a DRBG — callers keep their
//! documented boot-local entropy substitutes in that case.

use crate::{Result, SyscallNumber, syscall2};

/// Largest fill the kernel serves per RngRequest call; larger asks are
/// rejected by the kernel, so the wrapper clamps instead of erroring.
pub const RNG_MAX_REQUEST_BYTES: usize = 4096;

/// Fill `buffer` with kernel entropy. Returns the number of bytes written
/// (up to `buffer.len()`, clamped to [`RNG_MAX_REQUEST_BYTES`]).
pub fn entropy(buffer: &mut [u8]) -> Result<usize> {
    let length = buffer.len().min(RNG_MAX_REQUEST_BYTES);
    if length == 0 {
        return Ok(0);
    }
    syscall2(
        SyscallNumber::RngRequest,
        buffer.as_mut_ptr() as u64,
        length as u64,
    )
    .map(|value| value as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_buffer_short_circuits_without_syscall() {
        let mut empty: [u8; 0] = [];
        // On the host the syscall doorbell is not installed; the empty
        // fast path must return before touching it.
        assert_eq!(entropy(&mut empty), Ok(0));
    }
}
