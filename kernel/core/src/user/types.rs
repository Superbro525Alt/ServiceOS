use crate::{
    capability::CapabilityError,
    memory::{MappingError, PhysicalAddress, VirtualAddress},
    object::KernelObjectRef,
    task::{self, AddressSpaceId, ThreadId},
};

const FLAT_IMAGE_MAGIC: [u8; 8] = *b"SOSUIMG\0";
pub(super) const FLAT_IMAGE_HEADER_LEN: usize = 72;
pub(super) const USER_STACK_PAGES: usize = 16;
pub(super) const USER_ENTRY_STACK_BIAS: u64 = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskExitStatus {
    Running,
    Exited { code: u64 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreparedUserAddressSpace {
    pub page_table_root: PhysicalAddress,
    pub image: LoadedUserImage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UserThreadLaunch {
    pub thread_id: ThreadId,
    pub page_table_root: PhysicalAddress,
    pub entry_point: u64,
    pub user_stack_pointer: u64,
}

#[derive(Clone, Copy)]
pub struct UserArchHooks {
    pub prepare_address_space:
        fn(&[u8]) -> Result<PreparedUserAddressSpace, AddressSpacePreparationError>,
    pub register_thread_launch: fn(UserThreadLaunch),
}

#[derive(Clone)]
pub struct SpawnedUserTask {
    pub task: KernelObjectRef,
    pub thread: KernelObjectRef,
    pub address_space_id: AddressSpaceId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddressSpacePreparationError {
    Mapping(MappingError),
    Load(LoadError),
    NotInitialized,
}

impl From<MappingError> for AddressSpacePreparationError {
    fn from(error: MappingError) -> Self {
        Self::Mapping(error)
    }
}

impl From<LoadError> for AddressSpacePreparationError {
    fn from(error: LoadError) -> Self {
        Self::Load(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpawnError {
    ObjectsUnavailable,
    TasksUnavailable,
    MemoryUnavailable,
    ImageResolverUnavailable,
    ImageNotFound,
    ArchHooksUnavailable,
    AddressSpace(AddressSpacePreparationError),
    Capability(CapabilityError),
    Scheduler(task::SchedulerError),
}

impl From<AddressSpacePreparationError> for SpawnError {
    fn from(error: AddressSpacePreparationError) -> Self {
        Self::AddressSpace(error)
    }
}

impl From<CapabilityError> for SpawnError {
    fn from(error: CapabilityError) -> Self {
        Self::Capability(error)
    }
}

impl From<task::SchedulerError> for SpawnError {
    fn from(error: task::SchedulerError) -> Self {
        Self::Scheduler(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FlatImageHeader {
    pub abi_version: u32,
    pub image_base: VirtualAddress,
    pub entry_offset: u64,
    pub file_size: usize,
    pub executable_limit: usize,
    pub writable_offset: usize,
    pub memory_size: usize,
    pub user_stack_top: VirtualAddress,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadedUserImage {
    pub entry_point: VirtualAddress,
    pub image_base: VirtualAddress,
    pub file_size: usize,
    pub mapped_image_bytes: usize,
    pub user_stack_top: VirtualAddress,
    pub mapped_stack_bytes: usize,
}

impl LoadedUserImage {
    pub fn initial_stack_pointer(&self) -> u64 {
        self.user_stack_top
            .as_u64()
            .saturating_sub(USER_ENTRY_STACK_BIAS)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoadError {
    Truncated,
    InvalidMagic,
    UnsupportedAbi,
    UnsupportedHeader,
    AddressAlignment,
    FrameExhausted,
    Mapping(MappingError),
}

impl From<MappingError> for LoadError {
    fn from(error: MappingError) -> Self {
        Self::Mapping(error)
    }
}

pub(super) fn flat_image_magic() -> [u8; 8] {
    FLAT_IMAGE_MAGIC
}
