use serviceos_abi::Handle;

use crate::{
    capability::{CapabilityRights, CapabilityView},
    object::KernelObjectRef,
    user,
};

use super::SyscallError;

pub(super) fn current_task() -> Result<KernelObjectRef, SyscallError> {
    user::current_task().ok_or(SyscallError::NotInitialized)
}

pub(super) fn resolve_object(
    task: &KernelObjectRef,
    handle: Handle,
    required: CapabilityRights,
) -> Result<CapabilityView, SyscallError> {
    task.task()
        .ok_or(SyscallError::NotInitialized)?
        .capability_space()
        .resolve(crate::capability::CapabilityHandle(handle), required)
        .map_err(super::handlers::map_capability_error)
}
