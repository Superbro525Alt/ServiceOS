use crate::{
    capability::CapabilityError,
    ipc::IpcError,
    user::{AddressSpacePreparationError, LoadError, SpawnError},
};

use super::super::SyscallError;

pub(crate) static DEBUG_LOG_WRITER: spin::Once<fn(&[u8])> = spin::Once::new();
pub(crate) static DEBUG_CONSOLE_READER: spin::Once<fn() -> Option<u8>> = spin::Once::new();
pub(crate) static DEBUG_CONSOLE_WRITER: spin::Once<fn(&[u8])> = spin::Once::new();

pub(crate) fn map_spawn_error(error: SpawnError) -> SyscallError {
    match error {
        SpawnError::ImageNotFound => SyscallError::NotFound,
        SpawnError::Capability(error) => map_capability_error(error),
        SpawnError::Scheduler(_) => SyscallError::Busy,
        SpawnError::AddressSpace(AddressSpacePreparationError::Load(LoadError::FrameExhausted))
        | SpawnError::AddressSpace(AddressSpacePreparationError::Mapping(
            crate::memory::MappingError::FrameAllocationFailed,
        )) => SyscallError::CapacityExceeded,
        SpawnError::AddressSpace(AddressSpacePreparationError::Load(
            LoadError::UnsupportedFormat
            | LoadError::UnsupportedAbi
            | LoadError::UnsupportedHeader
            | LoadError::UnsupportedMachine
            | LoadError::UnsupportedRelocation
            | LoadError::KernelAbiTooNew
            | LoadError::DependencyInvalid,
        )) => SyscallError::Unsupported,
        SpawnError::AddressSpace(AddressSpacePreparationError::Load(
            LoadError::DependencyUnavailable,
        )) => SyscallError::NotFound,
        SpawnError::AddressSpace(AddressSpacePreparationError::Load(
            LoadError::Truncated | LoadError::InvalidMagic | LoadError::AddressAlignment,
        ))
        | SpawnError::AddressSpace(AddressSpacePreparationError::Load(LoadError::Mapping(
            crate::memory::MappingError::Unsupported,
        ))) => SyscallError::InvalidArgument,
        SpawnError::AddressSpace(AddressSpacePreparationError::Load(LoadError::Mapping(_)))
        | SpawnError::AddressSpace(AddressSpacePreparationError::Mapping(_)) => SyscallError::Busy,
        SpawnError::ObjectsUnavailable
        | SpawnError::TasksUnavailable
        | SpawnError::MemoryUnavailable
        | SpawnError::ImageResolverUnavailable
        | SpawnError::ArchHooksUnavailable
        | SpawnError::AddressSpace(AddressSpacePreparationError::NotInitialized) => {
            SyscallError::NotInitialized
        }
    }
}

pub(crate) fn map_capability_error(error: CapabilityError) -> SyscallError {
    match error {
        CapabilityError::InvalidHandle => SyscallError::NotFound,
        CapabilityError::HandleSpaceExhausted => SyscallError::CapacityExceeded,
        CapabilityError::RightsViolation { .. }
        | CapabilityError::DuplicateForbidden
        | CapabilityError::TransferForbidden
        | CapabilityError::RequestedRightsExceedSource => SyscallError::PermissionDenied,
    }
}

pub(crate) fn map_ipc_error(error: IpcError) -> SyscallError {
    match error {
        IpcError::Capability(error) => map_capability_error(error),
        IpcError::EndpointNotReady | IpcError::EndpointClosed => SyscallError::Busy,
        IpcError::BufferShapeInvalid
        | IpcError::ObjectKindMismatch
        | IpcError::InvalidReplyEndpoint => SyscallError::InvalidArgument,
        IpcError::QueueEmpty => SyscallError::QueueEmpty,
        IpcError::QueueFull { .. }
        | IpcError::MessageTooLarge { .. }
        | IpcError::TooManyTransfers { .. } => SyscallError::CapacityExceeded,
    }
}
