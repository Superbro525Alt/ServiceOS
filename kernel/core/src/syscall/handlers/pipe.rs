use alloc::sync::Arc;
use serviceos_abi::{Handle, HandlePair, PIPE_FLAG_NONBLOCK};

use super::{
    super::{
        SyscallAction, SyscallContext, SyscallError, SyscallReturn,
        resolve::{current_task, resolve_object},
        user_mut, user_slice, user_slice_mut,
    },
    common::DEBUG_LOG_WRITER,
};
use crate::{
    capability::{CapabilityHandle, CapabilityResolver, CapabilityRights, CapabilityView},
    object::{ObjectKind, PipeReadOutcome, PipeWriteOutcome},
};

const PIPE_READER_RIGHTS: CapabilityRights = CapabilityRights::READ.union(CapabilityRights::WAIT);
const PIPE_WRITER_RIGHTS: CapabilityRights = CapabilityRights::WRITE.union(CapabilityRights::WAIT);

/// Resolves a handle that must reference a pipe object.
fn resolve_pipe_view(
    task: &crate::object::KernelObjectRef,
    handle: Handle,
    required: CapabilityRights,
) -> Result<CapabilityView, SyscallError> {
    let view = resolve_object(task, handle, required)?;
    match view.object.kind() {
        ObjectKind::Pipe => Ok(view),
        _ => Err(SyscallError::InvalidArgument),
    }
}

pub(crate) fn handle_pipe_create(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(task) = current_task.task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let Some(objects) = crate::object::model() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    // Validate the user's out-pointer before installing anything so a bad
    // address cannot strand the pair of handles about to be created.
    if unsafe { user_mut::<HandlePair>(context.arguments[0]) }.is_err() {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    }

    let pipe = objects.registry().create_pipe();
    let reader_handle =
        match task
            .capability_space()
            .install(Arc::clone(&pipe), PIPE_READER_RIGHTS, None)
        {
            Ok(handle) => handle,
            Err(error) => return SyscallReturn::error(map_capability_error_pub(error)),
        };
    let writer_handle =
        match task
            .capability_space()
            .install(Arc::clone(&pipe), PIPE_WRITER_RIGHTS, None)
        {
            Ok(handle) => handle,
            Err(error) => {
                // Roll back the reader install so no half-pair leaks.
                let _ = task.capability_space().close(reader_handle);
                return SyscallReturn::error(map_capability_error_pub(error));
            }
        };

    // The pointer was validated above and nothing unmaps in between, so this
    // write cannot fault; both handles stay installed.
    if let Ok(pair_out) = unsafe { user_mut::<HandlePair>(context.arguments[0]) } {
        *pair_out = HandlePair {
            first: reader_handle.0,
            second: writer_handle.0,
        };
    }

    if let Some(writer) = DEBUG_LOG_WRITER.get() {
        writer(b"pipe: created reader+writer pair\r\n");
    }
    SyscallReturn::success(0)
}

fn map_capability_error_pub(error: crate::capability::CapabilityError) -> SyscallError {
    super::common::map_capability_error(error)
}

pub(crate) fn handle_pipe_read(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let view = match resolve_pipe_view(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::READ,
    ) {
        Ok(view) => view,
        Err(error) => return SyscallReturn::error(error),
    };
    let object = view.object;
    let Some(pipe) = object.pipe() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    let length = context.arguments[2] as usize;
    let flags = context.arguments[3] as u32;
    if length == 0 {
        return SyscallReturn::success(0);
    }
    let Ok(buffer) = (unsafe { user_slice_mut(context.arguments[1], length) }) else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    match pipe.read(buffer) {
        PipeReadOutcome::Bytes(0) => {
            if flags & PIPE_FLAG_NONBLOCK != 0 {
                SyscallReturn::error(SyscallError::QueueEmpty)
            } else {
                SyscallReturn::error_with_action(
                    SyscallError::QueueEmpty,
                    SyscallAction::BlockCurrentThreadOnObject {
                        object: object.id(),
                    },
                )
            }
        }
        PipeReadOutcome::Bytes(count) => {
            // Freed ring space may release blocked writers waiting on the
            // same object id.
            let _ = crate::task::notify_object_ready(object.id());
            SyscallReturn::success(count as u64)
        }
        PipeReadOutcome::EndOfStream => {
            let _ = crate::task::notify_object_ready(object.id());
            SyscallReturn::success(0)
        }
    }
}

pub(crate) fn handle_pipe_write(context: &SyscallContext) -> SyscallReturn {
    let Ok(current_task) = current_task() else {
        return SyscallReturn::error(SyscallError::NotInitialized);
    };
    let view = match resolve_pipe_view(
        &current_task,
        context.arguments[0] as Handle,
        CapabilityRights::WRITE,
    ) {
        Ok(view) => view,
        Err(error) => return SyscallReturn::error(error),
    };
    let object = view.object;
    let Some(pipe) = object.pipe() else {
        return SyscallReturn::error(SyscallError::InvalidArgument);
    };

    let length = context.arguments[2] as usize;
    let flags = context.arguments[3] as u32;
    let buffer: &[u8] = if length == 0 {
        &[]
    } else {
        match unsafe { user_slice(context.arguments[1], length) } {
            Ok(slice) => slice,
            Err(error) => return SyscallReturn::error(error),
        }
    };

    match pipe.write(buffer) {
        PipeWriteOutcome::BrokenPipe => SyscallReturn::error(SyscallError::BrokenPipe),
        PipeWriteOutcome::WouldBlock => {
            if flags & PIPE_FLAG_NONBLOCK != 0 {
                SyscallReturn::error(SyscallError::QueueEmpty)
            } else {
                SyscallReturn::error_with_action(
                    SyscallError::QueueEmpty,
                    SyscallAction::BlockCurrentThreadOnObject {
                        object: object.id(),
                    },
                )
            }
        }
        PipeWriteOutcome::Bytes(count) => {
            // New ring data releases blocked readers on the same object id.
            let _ = crate::task::notify_object_ready(object.id());
            SyscallReturn::success(count as u64)
        }
    }
}

/// Updates pipe side refcounts after a duplicate of a pipe handle was
/// installed, using the new handle's effective rights.
pub(crate) fn note_pipe_handle_duplicated(
    task: &crate::object::KernelObjectRef,
    handle: CapabilityHandle,
) {
    let Some(descriptor) = task
        .task()
        .and_then(|task| task.capability_space().resolve_descriptor(handle))
    else {
        return;
    };
    let Ok(view) = resolve_object(task, handle.0, CapabilityRights::NONE) else {
        return;
    };
    if view.object.kind() != ObjectKind::Pipe {
        return;
    }
    let Some(pipe) = view.object.pipe() else {
        return;
    };
    if descriptor.rights.contains(CapabilityRights::READ) {
        pipe.add_reader();
    }
    if descriptor.rights.contains(CapabilityRights::WRITE) {
        pipe.add_writer();
    }
}

/// Updates pipe side refcounts right before a pipe handle closes. Returns
/// whether waiters should be nudged (last writer closed -> readers see EOF;
/// last reader closed -> writers see broken pipe).
pub(crate) fn note_pipe_handle_closed(
    task: &crate::object::KernelObjectRef,
    handle: CapabilityHandle,
) -> bool {
    let Some(descriptor) = task
        .task()
        .and_then(|task| task.capability_space().resolve_descriptor(handle))
    else {
        return false;
    };
    let Ok(view) = resolve_object(task, handle.0, CapabilityRights::NONE) else {
        return false;
    };
    if view.object.kind() != ObjectKind::Pipe {
        return false;
    }
    let object = view.object;
    let Some(pipe) = object.pipe() else {
        return false;
    };

    let mut changed = false;
    if descriptor.rights.contains(CapabilityRights::WRITE) && pipe.close_writer() {
        changed = true;
    }
    if descriptor.rights.contains(CapabilityRights::READ) && pipe.close_reader() {
        changed = true;
    }
    if changed {
        let _ = crate::task::notify_object_ready(object.id());
    }
    changed
}
