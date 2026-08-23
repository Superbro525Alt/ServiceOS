use serviceos_abi::{Handle, TaskStateCode, TaskStatus as AbiTaskStatus};

use super::{
    super::{
        SyscallContext, SyscallError, SyscallReturn,
        resolve::{current_task, resolve_object},
    },
    common::{map_capability_error, map_spawn_error},
};
use crate::{
    capability::{CapabilityHandle, CapabilityResolver, CapabilityRights, TransferMode},
    task::TaskRole,
    user,
    user::MAX_FLAT_DEPENDENCIES,
};

pub(crate) fn handle_service_spawn(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(task) = current_task.task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let authority = match resolve_object(
        &current_task,
        context.arguments[1] as Handle,
        CapabilityRights::bootstrap(),
    ) {
        Ok(authority) => authority,
        Err(error) => return SyscallReturn::error(error),
    };
    if authority.object.bootstrap_capability().is_none() {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    }

    let bootstrap_transfer = if context.arguments[2] == 0 {
        None
    } else {
        let handle = CapabilityHandle(context.arguments[2] as Handle);
        let Some(descriptor) = task.capability_space().resolve_descriptor(handle) else {
            return SyscallReturn::error(SyscallError::NotFound);
        };
        match task.capability_space().prepare_transfer(
            handle,
            descriptor.rights,
            TransferMode::Move,
        ) {
            Ok(transfer) => Some(transfer),
            Err(error) => return SyscallReturn::error(map_capability_error(error)),
        }
    };

    let spawned = match user::spawn_builtin_task(
        context.arguments[0] as u32,
        TaskRole::SystemService,
        bootstrap_transfer.clone(),
    ) {
        Ok(spawned) => spawned,
        Err(error) => {
            // The Move already removed the source handle; give it back so a
            // failed spawn does not consume the caller's capability.
            if let Some(transfer) = &bootstrap_transfer {
                task.capability_space().rollback_moved(transfer);
            }
            return SyscallReturn::error(map_spawn_error(error));
        }
    };

    match task
        .capability_space()
        .install(spawned.task, CapabilityRights::task(), None)
    {
        Ok(handle) => SyscallReturn::success(handle.0 as u64),
        Err(error) => SyscallReturn::error(map_capability_error(error)),
    }
}

pub(crate) fn handle_task_status(context: &SyscallContext) -> SyscallReturn {
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
    let Some(target_task) = object.task() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Ok(status_out) = (unsafe { super::super::user_mut::<AbiTaskStatus>(context.arguments[1]) })
    else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    *status_out = match target_task.exit_status() {
        user::TaskExitStatus::Running => AbiTaskStatus {
            state: TaskStateCode::Running,
            exit_code: 0,
        },
        user::TaskExitStatus::Exited { code } => AbiTaskStatus {
            state: TaskStateCode::Exited,
            exit_code: code,
        },
        user::TaskExitStatus::Faulted { code } => AbiTaskStatus {
            state: TaskStateCode::Faulted,
            exit_code: code,
        },
    };

    SyscallReturn::success(0)
}

pub(crate) fn handle_task_loaded_libraries(context: &SyscallContext) -> SyscallReturn {
    use serviceos_abi::TaskLoadedLibrary;

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
    let Some(target_task) = object.task() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Some(address_space_id) = target_task.address_space() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let Some(image) = user::loaded_image_for(address_space_id) else {
        return SyscallReturn::error(SyscallError::NotFound);
    };

    let capacity = context.arguments[2] as usize;
    if capacity < image.library_count {
        return SyscallReturn::error(SyscallError::CapacityExceeded);
    }
    if image.library_count > 0 {
        let mut staged = [TaskLoadedLibrary {
            image_id: 0,
            _pad: 0,
            base: 0,
            mapped_bytes: 0,
        }; MAX_FLAT_DEPENDENCIES];
        for (slot, record) in staged.iter_mut().zip(image.libraries.iter()) {
            *slot = TaskLoadedLibrary {
                image_id: record.image_id,
                _pad: 0,
                base: record.base.as_u64(),
                mapped_bytes: record.mapped_bytes as u64,
            };
        }
        let byte_len = core::mem::size_of::<TaskLoadedLibrary>() * image.library_count;
        let Ok(out_bytes) =
            (unsafe { super::super::user_slice_mut(context.arguments[1], byte_len) })
        else {
            return SyscallReturn::error(SyscallError::InvalidArgument);
        };
        let src = core::ptr::slice_from_raw_parts(staged.as_ptr().cast::<u8>(), byte_len);
        out_bytes.copy_from_slice(unsafe { &*src });
    }

    SyscallReturn::success(image.library_count as u64)
}

pub(crate) fn handle_task_spawn_image(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(task) = current_task.task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let authority = match task.capability_space().resolve(
        CapabilityHandle(context.arguments[1] as Handle),
        CapabilityRights::bootstrap(),
    ) {
        Ok(view) => view,
        Err(error) => return SyscallReturn::error(map_capability_error(error)),
    };
    if authority.object.bootstrap_capability().is_none() {
        return SyscallReturn::error(SyscallError::PermissionDenied);
    }
    let image_view = match resolve_object(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::READ,
    ) {
        Ok(view) => view,
        Err(error) => return SyscallReturn::error(error),
    };
    let Some(image_object) = image_view.object.memory_object() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };
    let image_len = image_object.info().size_bytes;
    let mut image_bytes = alloc::vec![0u8; image_len];
    let copied = image_object.read(0, &mut image_bytes);
    image_bytes.truncate(copied);

    let bootstrap_transfer = if context.arguments[2] == 0 {
        None
    } else {
        match task.capability_space().prepare_transfer(
            CapabilityHandle(context.arguments[2] as Handle),
            CapabilityRights::channel_endpoint(),
            TransferMode::Copy,
        ) {
            Ok(transfer) => Some(transfer),
            Err(error) => return SyscallReturn::error(map_capability_error(error)),
        }
    };

    let spawned =
        match user::spawn_image_bytes(&image_bytes, TaskRole::UserService, bootstrap_transfer) {
            Ok(spawned) => spawned,
            Err(error) => return SyscallReturn::error(map_spawn_error(error)),
        };
    match task
        .capability_space()
        .install(spawned.task, CapabilityRights::task(), None)
    {
        Ok(handle) => SyscallReturn::success(handle.0 as u64),
        Err(error) => SyscallReturn::error(map_capability_error(error)),
    }
}
