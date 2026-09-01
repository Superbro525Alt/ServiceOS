use serviceos_abi::{
    Handle, MemoryMapRequest, MemoryObjectInfo as AbiMemoryObjectInfo, memory_map_flags,
};

use super::{
    super::{
        SyscallContext, SyscallError, SyscallReturn,
        resolve::{current_task, resolve_object},
        user_mut, user_ref, user_slice, user_slice_mut,
    },
    common::map_capability_error,
};
use crate::{
    capability::CapabilityRights,
    fault::{self, FaultType},
    memory::{MappingFlags, PAGE_SIZE_BYTES},
    user,
};

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
    let object = objects.registry().create_memory_object(
        size_bytes,
        writable,
        crate::object::DmaSafety::Unsafe,
    );

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
        // Writes never fetch a device backing; the DMA policy cannot be
        // violated here. Arm exists for exhaustiveness.
        Err(crate::object::MemoryAccessError::DmaPolicyViolation) => {
            SyscallReturn::error(SyscallError::Busy)
        }
    }
}

pub(crate) fn handle_memory_map(context: &SyscallContext) -> SyscallReturn {
    map_memory_object(
        context,
        context.arguments[0] as Handle,
        MemoryMapRequest {
            offset_bytes: 0,
            length_bytes: 0,
            address_hint: 0,
            flags: if context.arguments[1] != 0 {
                memory_map_flags::WRITABLE
            } else {
                0
            },
            reserved: 0,
        },
    )
}

pub(crate) fn handle_memory_info(context: &SyscallContext) -> SyscallReturn {
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
    let Ok(info_out) = (unsafe { user_mut::<AbiMemoryObjectInfo>(context.arguments[1]) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    *info_out = AbiMemoryObjectInfo {
        size_bytes: memory.info().size_bytes,
        page_count: memory.info().page_count,
        writable: memory.info().writable,
    };
    SyscallReturn::success(0)
}

pub(crate) fn handle_memory_map_range(context: &SyscallContext) -> SyscallReturn {
    let Ok(request) = (unsafe { user_ref::<MemoryMapRequest>(context.arguments[1]) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    map_memory_object(context, context.arguments[0] as Handle, *request)
}

pub(crate) fn handle_memory_unmap(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let object = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::READ.union(CapabilityRights::MAP),
    ) {
        Ok(view) => view.object,
        Err(error) => return SyscallReturn::error(error),
    };
    let _memory = match object.memory_object() {
        Some(memory) => memory,
        None => return SyscallReturn::error(SyscallError::InvalidArgument),
    };

    let Ok(address) = usize::try_from(context.arguments[1]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(length) = usize::try_from(context.arguments[2]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    let Some(task) = current_task.task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(address_space_id) = task.address_space() else {
        return SyscallReturn::error(SyscallError::Unsupported);
    };
    let Some(runtime) = user::runtime() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(hooks) = user::arch_hooks() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };

    let page_size = PAGE_SIZE_BYTES as usize;
    let page_count = length.div_ceil(page_size);
    let Some(span_bytes) = (page_count as u64).checked_mul(PAGE_SIZE_BYTES) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    if !runtime.contains_reserved_mapping_range(address_space_id, address as u64, span_bytes) {
        return SyscallReturn::error(SyscallError::PermissionDenied);
    }

    match (hooks.unmap_memory_range)(
        address_space_id,
        crate::memory::VirtualAddress::new(address as u64),
        page_count,
    ) {
        Ok(()) => SyscallReturn::success(0),
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

pub(crate) fn handle_memory_protect(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let object = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::READ.union(CapabilityRights::MAP),
    ) {
        Ok(view) => view.object,
        Err(error) => return SyscallReturn::error(error),
    };
    let _memory = match object.memory_object() {
        Some(memory) => memory,
        None => return SyscallReturn::error(SyscallError::InvalidArgument),
    };

    let Ok(address) = usize::try_from(context.arguments[1]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(length) = usize::try_from(context.arguments[2]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let protect_flags = context.arguments[3];

    let Some(task) = current_task.task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(address_space_id) = task.address_space() else {
        return SyscallReturn::error(SyscallError::Unsupported);
    };
    let Some(runtime) = user::runtime() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(hooks) = user::arch_hooks() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };

    let page_size = PAGE_SIZE_BYTES as usize;
    let page_count = length.div_ceil(page_size);
    let Some(span_bytes) = (page_count as u64).checked_mul(PAGE_SIZE_BYTES) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    if !runtime.contains_reserved_mapping_range(address_space_id, address as u64, span_bytes) {
        return SyscallReturn::error(SyscallError::PermissionDenied);
    }

    let mut flags = MappingFlags::empty();
    if protect_flags & 1 != 0 {
        flags |= MappingFlags::WRITABLE;
    }
    if protect_flags & 2 != 0 {
        flags |= MappingFlags::EXECUTABLE;
    }
    if protect_flags & 4 != 0 {
        flags |= MappingFlags::USER_ACCESSIBLE;
    }

    match (hooks.update_memory_protection)(
        address_space_id,
        crate::memory::VirtualAddress::new(address as u64),
        page_count,
        flags,
    ) {
        Ok(()) => SyscallReturn::success(0),
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

pub(crate) fn handle_memory_query(context: &SyscallContext) -> SyscallReturn {
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

    let Ok(address) = usize::try_from(context.arguments[1]) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    let Some(task) = current_task.task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(address_space_id) = task.address_space() else {
        return SyscallReturn::error(SyscallError::Unsupported);
    };
    let Some(hooks) = user::arch_hooks() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };

    let virt_addr = crate::memory::VirtualAddress::new(address as u64);
    let Some(_memory) = object.memory_object() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    let page_size = PAGE_SIZE_BYTES as usize;

    let Some(phys_addr) = (hooks.translate_address)(address_space_id, virt_addr) else {
        return SyscallReturn::error(SyscallError::NotFound);
    };

    let Ok(info_out) = (unsafe { user_mut::<MemoryMapRequest>(context.arguments[2]) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    let page_index = address / page_size;
    let offset = page_index * page_size;

    *info_out = MemoryMapRequest {
        offset_bytes: offset,
        length_bytes: page_size,
        address_hint: phys_addr.as_u64(),
        flags: 0,
        reserved: 0,
    };
    SyscallReturn::success(0)
}

pub(crate) fn handle_fault_handler_register(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(scheduler) = crate::task::system().map(|s| s.scheduler()) else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let thread_id = match scheduler.current_thread() {
        Some(id) => id,
        None => return SyscallReturn::error(SyscallError::NotInitialized),
    };

    let fault_type_value = context.arguments[0];
    let endpoint_handle = context.arguments[1] as Handle;

    let fault_type = match fault_type_value {
        0 => FaultType::InvalidOpcode,
        1 => FaultType::PageFault,
        2 => FaultType::GeneralProtection,
        3 => FaultType::Breakpoint,
        n => FaultType::Other(n as u8),
    };

    let view = match resolve_object(&current_task, endpoint_handle, CapabilityRights::SEND) {
        Ok(view) => view,
        Err(error) => return SyscallReturn::error(error),
    };

    match fault::register_fault_handler(fault_type, thread_id, view.object.id()) {
        Ok(()) => SyscallReturn::success(0),
        Err(fault::FaultRegistrationError::AlreadyRegistered) => {
            SyscallReturn::error(SyscallError::Busy)
        }
        Err(fault::FaultRegistrationError::NotRegistered) => {
            SyscallReturn::error(SyscallError::NotFound)
        }
    }
}

pub(crate) fn handle_fault_handler_unregister(context: &SyscallContext) -> SyscallReturn {
    let fault_type_value = context.arguments[0];

    let fault_type = match fault_type_value {
        0 => FaultType::InvalidOpcode,
        1 => FaultType::PageFault,
        2 => FaultType::GeneralProtection,
        3 => FaultType::Breakpoint,
        n => FaultType::Other(n as u8),
    };

    match fault::unregister_fault_handler(&fault_type) {
        Ok(()) => SyscallReturn::success(0),
        Err(fault::FaultRegistrationError::AlreadyRegistered) => {
            SyscallReturn::error(SyscallError::Busy)
        }
        Err(fault::FaultRegistrationError::NotRegistered) => {
            SyscallReturn::error(SyscallError::NotFound)
        }
    }
}

fn map_memory_object(
    _context: &SyscallContext,
    handle: Handle,
    request: MemoryMapRequest,
) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let view = match resolve_object(
        &current_task,
        handle,
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
    let writable = (request.flags & memory_map_flags::WRITABLE) != 0;
    if writable && !view.rights.contains(CapabilityRights::WRITE) {
        return SyscallReturn::error(SyscallError::PermissionDenied);
    }
    if (request.flags & memory_map_flags::FIXED) != 0 {
        return SyscallReturn::error(SyscallError::Unsupported);
    }
    let page_size = PAGE_SIZE_BYTES as usize;
    if request.offset_bytes % page_size != 0 {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    }
    let info = memory.info();
    if request.offset_bytes > info.size_bytes {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    }
    let length_bytes = if request.length_bytes == 0 {
        info.size_bytes.saturating_sub(request.offset_bytes)
    } else {
        request.length_bytes
    };
    if length_bytes == 0 {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    }
    let end = match request.offset_bytes.checked_add(length_bytes) {
        Some(end) if end <= info.size_bytes => end,
        _ => return SyscallReturn::error(SyscallError::InvalidArgument),
    };
    let page_offset = request.offset_bytes / page_size;
    let page_count = length_bytes.div_ceil(page_size);
    let Some(runtime) = user::runtime() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(hooks) = user::arch_hooks() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(base) = runtime.reserve_mapping_range(address_space_id, page_count * page_size) else {
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
        // map-range is a CPU mapping path: it is allowed for any DMA class,
        // so a declared-Contiguous object whose materialized frames turn out
        // discontiguous surfaces here as a resource error, not a violation.
        Err(crate::object::MemoryAccessError::DmaPolicyViolation) => {
            return SyscallReturn::error(SyscallError::Busy);
        }
    };
    let Some(mapped_frames) = frames.get(page_offset..page_offset + page_count) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let _ = end;

    match (hooks.map_memory_object)(address_space_id, base, mapped_frames, writable) {
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
