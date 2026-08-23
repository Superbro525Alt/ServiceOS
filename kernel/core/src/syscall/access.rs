use super::SyscallError;
use crate::memory::USER_SPACE_END;

/// Lowest virtual address from which userspace images and buffers may be
/// backed. The image builder places every userspace image at
/// `0x0000_4000_0000_0000` with the stack top below `USER_SPACE_END`, so any
/// pointer outside this window cannot name a legitimate userspace buffer.
/// This keeps syscalls from dereferencing kernel mappings (higher-half heap,
/// direct map, page-table frames under the identity map) on behalf of a task.
const USER_POINTER_MIN: u64 = 0x0000_4000_0000_0000;

fn validate_user_range(address: u64, length: u64) -> Result<(), SyscallError> {
    let window_end = USER_SPACE_END.as_u64();
    if address < USER_POINTER_MIN || address >= window_end {
        return Err(SyscallError::InvalidArgument);
    }
    let end = address
        .checked_add(length)
        .ok_or(SyscallError::InvalidArgument)?;
    if end > window_end {
        return Err(SyscallError::InvalidArgument);
    }
    Ok(())
}

pub unsafe fn user_ref<T>(address: u64) -> Result<&'static T, SyscallError> {
    if address == 0 || address as usize % core::mem::align_of::<T>() != 0 {
        return Err(SyscallError::InvalidArgument);
    }
    validate_user_range(address, core::mem::size_of::<T>() as u64)?;

    Ok(unsafe { &*(address as *const T) })
}

pub unsafe fn user_mut<T>(address: u64) -> Result<&'static mut T, SyscallError> {
    if address == 0 || address as usize % core::mem::align_of::<T>() != 0 {
        return Err(SyscallError::InvalidArgument);
    }
    validate_user_range(address, core::mem::size_of::<T>() as u64)?;

    Ok(unsafe { &mut *(address as *mut T) })
}

pub unsafe fn user_slice(address: u64, len: usize) -> Result<&'static [u8], SyscallError> {
    if address == 0 {
        return Err(SyscallError::InvalidArgument);
    }
    validate_user_range(address, len as u64)?;

    Ok(unsafe { core::slice::from_raw_parts(address as *const u8, len) })
}

pub unsafe fn user_slice_mut(address: u64, len: usize) -> Result<&'static mut [u8], SyscallError> {
    if address == 0 {
        return Err(SyscallError::InvalidArgument);
    }
    validate_user_range(address, len as u64)?;

    Ok(unsafe { core::slice::from_raw_parts_mut(address as *mut u8, len) })
}

#[cfg(test)]
mod tests {
    use super::*;

    const IMAGE_BASE: u64 = 0x0000_4000_0000_0000;
    const STACK_TOP: u64 = 0x0000_7fff_ffff_f000;
    const KERNEL_HEAP: u64 = 0xffff_c100_0000_0000;
    const IDENTITY_PT: u64 = 0x0000_1000;

    #[test]
    fn rejects_kernel_and_identity_ranges() {
        assert_eq!(
            unsafe { user_slice(KERNEL_HEAP, 16) }.err(),
            Some(SyscallError::InvalidArgument)
        );
        assert_eq!(
            unsafe { user_slice(IDENTITY_PT, 16) }.err(),
            Some(SyscallError::InvalidArgument)
        );
    }

    #[test]
    fn accepts_image_window_and_rejects_overflow() {
        // Last valid byte of the window is USER_SPACE_END - 1.
        assert!(unsafe { user_slice(STACK_TOP - 16, 16) }.is_ok());
        // A range ending exactly at the window edge is still inside.
        assert!(unsafe { user_slice(STACK_TOP, 4096) }.is_ok());
        // One byte past the edge must be rejected.
        assert_eq!(
            unsafe { user_slice(STACK_TOP - 8, 4105) }.err(),
            Some(SyscallError::InvalidArgument)
        );
        // Wrapped length must be rejected.
        assert_eq!(
            unsafe { user_slice(u64::MAX - 8, 32) }.err(),
            Some(SyscallError::InvalidArgument)
        );
    }

    #[test]
    fn sized_accessors_check_struct_span() {
        let ok = IMAGE_BASE;
        assert!(unsafe { user_ref::<[u64; 128]>(ok) }.is_ok());
        // A wide struct ending beyond the window must be rejected.
        let near_edge = 0x0000_8000_0000_0000u64 - 512;
        assert_eq!(
            unsafe { user_mut::<[u64; 128]>(near_edge) }.err(),
            Some(SyscallError::InvalidArgument)
        );
    }

    #[test]
    fn window_matches_builder_image_base() {
        assert_eq!(USER_POINTER_MIN, IMAGE_BASE);
    }
}
