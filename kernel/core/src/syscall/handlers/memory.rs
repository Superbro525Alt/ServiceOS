use serviceos_abi::Handle;

use super::{
    common::map_capability_error,
    super::{
        SyscallContext, SyscallError, SyscallReturn, resolve::{current_task, resolve_object},
        user_slice, user_slice_mut,
    },
};
use crate::{capability::CapabilityRights, user};

pub(crate) fn handle_memory_read(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let object = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::READ,
    ) {
        Ok(view) => view.object,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(memory) = object.memory_object() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(offset) = usize::try_from(context.arguments[1]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(length) = usize::try_from(context.arguments[3]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(destination) = (unsafe { user_slice_mut(context.arguments[2], length) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    SyscallReturn::success(memory.read(offset, destination) as u64)
}

pub(crate) fn handle_memory_create(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(task) = current_task.task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(objects) = crate::object::model() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Ok(size_bytes) = usize::try_from(context.arguments[0]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let writable = context.arguments[1] != 0;
    let object = objects
        .registry()
        .create_memory_object(size_bytes, writable);

    match task
        .capability_space()
        .install(object, CapabilityRights::memory_object(), None)
    {
        Ok(handle) => SyscallReturn::success(handle.0 as u64),
        Err(error) => SyscallReturn::error(map_capability_error(error)),
    }
}

pub(crate) fn handle_memory_write(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let object = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::WRITE,
    ) {
        Ok(view) => view.object,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(memory) = object.memory_object() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(offset) = usize::try_from(context.arguments[1]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(length) = usize::try_from(context.arguments[3]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(source) = (unsafe { user_slice(context.arguments[2], length) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    match memory.write(offset, source) {
        Ok(written) => SyscallReturn::success(written as u64),
        Err(crate::object::MemoryAccessError::ReadOnly) => {
            SyscallReturn::error(SyscallError::PermissionDenied)
        }
        Err(crate::object::MemoryAccessError::Busy) => SyscallReturn::error(SyscallError::Busy),
        Err(crate::object::MemoryAccessError::Unsupported) => {
            SyscallReturn::error(SyscallError::Unsupported)
        }
    }
}

pub(crate) fn handle_memory_map(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let view = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::READ.union(CapabilityRights::MAP),
    ) {
        Ok(view) => view,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(task) = current_task.task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(address_space_id) = task.address_space() else {
        return SyscallReturn::error(SyscallError::Unsupported);
    };
    let Some(memory) = view.object.memory_object() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let writable = context.arguments[1] != 0;
    if writable && !view.rights.contains(CapabilityRights::WRITE) {
        return SyscallReturn::error(SyscallError::PermissionDenied);
    }
    let Some(runtime) = user::runtime() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(hooks) = user::arch_hooks() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(base) = runtime.reserve_mapping_range(address_space_id, memory.info().size_bytes)
    else {
        return SyscallReturn::error(SyscallError::Busy);
    };
    let frames = match memory.page_frames() {
        Ok(frames) => frames,
        Err(crate::object::MemoryAccessError::ReadOnly) => {
            return SyscallReturn::error(SyscallError::PermissionDenied);
        }
        Err(crate::object::MemoryAccessError::Busy) => {
            return SyscallReturn::error(SyscallError::Busy);
        }
        Err(crate::object::MemoryAccessError::Unsupported) => {
            return SyscallReturn::error(SyscallError::Unsupported);
        }
    };

    match (hooks.map_memory_object)(address_space_id, base, frames.as_ref(), writable) {
        Ok(()) => SyscallReturn::success(base.as_u64()),
        Err(crate::memory::MappingError::AddressAlignment) => {
            SyscallReturn::error(SyscallError::InvalidArgument)
        }
        Err(crate::memory::MappingError::AlreadyMapped) => SyscallReturn::error(SyscallError::Busy),
        Err(crate::memory::MappingError::FrameAllocationFailed) => {
            SyscallReturn::error(SyscallError::Busy)
        }
        Err(crate::memory::MappingError::Unsupported) => {
            SyscallReturn::error(SyscallError::Unsupported)
        }
    }
}
