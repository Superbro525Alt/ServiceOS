use super::SyscallError;

pub unsafe fn user_ref<T>(address: u64) -> Result<&'static T, SyscallError> {
    if address == 0 || address as usize % core::mem::align_of::<T>() != 0 {
        return Err(SyscallError::InvalidArgument);
    }

    Ok(unsafe { &*(address as *const T) })
}

pub unsafe fn user_mut<T>(address: u64) -> Result<&'static mut T, SyscallError> {
    if address == 0 || address as usize % core::mem::align_of::<T>() != 0 {
        return Err(SyscallError::InvalidArgument);
    }

    Ok(unsafe { &mut *(address as *mut T) })
}

pub unsafe fn user_slice(address: u64, len: usize) -> Result<&'static [u8], SyscallError> {
    if address == 0 {
        return Err(SyscallError::InvalidArgument);
    }

    Ok(unsafe { core::slice::from_raw_parts(address as *const u8, len) })
}

pub unsafe fn user_slice_mut(address: u64, len: usize) -> Result<&'static mut [u8], SyscallError> {
    if address == 0 {
        return Err(SyscallError::InvalidArgument);
    }

    Ok(unsafe { core::slice::from_raw_parts_mut(address as *mut u8, len) })
}
