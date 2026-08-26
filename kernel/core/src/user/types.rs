use crate::{
    capability::CapabilityError,
    memory::{MappingError, MappingFlags, PhysicalAddress, VirtualAddress},
    object::KernelObjectRef,
    task::{self, AddressSpaceId, ThreadId},
};

const FLAT_IMAGE_MAGIC: [u8; 8] = *b"SOSUIMG\0";
pub(super) const FLAT_IMAGE_HEADER_LEN: usize = 72;
/// Extended (additive) header layout. The legacy 72-byte header is unchanged;
/// v2 images set `header_len = FLAT_IMAGE_HEADER_LEN_V2` and append the
/// policy/dependency/segment-table fields documented in `shared/bundle`.
pub(super) const FLAT_IMAGE_HEADER_LEN_V2: usize = 280;
pub(super) const USER_STACK_PAGES: usize = 256;
pub(super) const USER_ENTRY_STACK_BIAS: u64 = 8;

/// ABI contract implemented by this kernel's loader. Images may declare a
/// higher `min_kernel_abi`; the loader rejects those with
/// [`LoadError::KernelAbiTooNew`].
pub const KERNEL_ABI_VERSION: u32 = 1;

/// Maximum companion library images a flat image may declare.
pub const MAX_FLAT_DEPENDENCIES: usize = 4;
/// Maximum explicit segment descriptors in the offset-table payload layout.
pub const MAX_FLAT_SEGMENTS: usize = 4;

/// Policy/descriptor flags stored at header offset 72 of an extended flat
/// image. Unknown bits are rejected so future policy cannot be silently
/// ignored.
pub mod flat_image_policy {
    /// Task requires a stack guard page below its stack. Parsed and surfaced
    /// to task inspection; guard-page enforcement is not yet implemented.
    pub const REQUIRE_STACK_GUARD: u32 = 1 << 0;
    /// Task opts out of address-space randomization. The kernel currently has
    /// no userspace ASLR, so this is accepted and ignored by design.
    pub const NO_ASLR: u32 = 1 << 1;
    /// Payload permissions come from the segment descriptor table instead of
    /// the single `executable_limit`/`writable_offset` split.
    pub const SEGMENT_TABLE: u32 = 1 << 2;

    pub const VALID_MASK: u32 = REQUIRE_STACK_GUARD | NO_ASLR | SEGMENT_TABLE;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskExitStatus {
    Running,
    Exited { code: u64 },
    Faulted { code: u64 },
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
    pub release_thread_runtime: fn(ThreadId),
    pub register_address_space: fn(AddressSpaceId, PhysicalAddress),
    pub release_address_space: fn(AddressSpaceId),
    pub map_memory_object:
        fn(AddressSpaceId, VirtualAddress, &[PhysicalAddress], bool) -> Result<(), MappingError>,
    pub unmap_memory_range: fn(AddressSpaceId, VirtualAddress, usize) -> Result<(), MappingError>,
    pub update_memory_protection:
        fn(AddressSpaceId, VirtualAddress, usize, MappingFlags) -> Result<(), MappingError>,
    pub translate_address: fn(AddressSpaceId, VirtualAddress) -> Option<PhysicalAddress>,
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

/// One declared companion library image in an extended flat image header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FlatDependencyRecord {
    /// BootStore image id of the companion library.
    pub image_id: u32,
    /// Hinted placement as a byte offset from the main image base. `0` means
    /// "no hint"; the loader picks the next free region above the main image.
    pub base_offset_hint: u64,
}

/// Explicit payload region descriptor for offset-table (`SEGMENT_TABLE`)
/// images. Offsets are page-aligned; regions may not overlap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FlatSegmentRecord {
    /// Offset from `image_base` where the segment is mapped.
    pub virtual_offset: u64,
    /// Offset from the start of the payload (just past the header) of the
    /// initialized bytes for this segment.
    pub file_offset: u64,
    pub file_size: u64,
    /// Bytes mapped for the segment; the tail past `file_size` is zero-filled.
    pub memory_size: u64,
    pub executable: bool,
    pub writable: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FlatImageHeader {
    pub abi_version: u32,
    /// Total byte length of the header itself (72 legacy, 280 extended). The
    /// payload starts at this offset in the image file.
    pub header_len: usize,
    pub image_base: VirtualAddress,
    pub entry_offset: u64,
    pub file_size: usize,
    pub executable_limit: usize,
    pub writable_offset: usize,
    pub memory_size: usize,
    pub user_stack_top: VirtualAddress,
    pub format_flags: u32,
    pub min_kernel_abi: u32,
    pub dependencies: [FlatDependencyRecord; MAX_FLAT_DEPENDENCIES],
    pub dependency_count: usize,
    pub segments: [FlatSegmentRecord; MAX_FLAT_SEGMENTS],
    pub segment_count: usize,
}

impl FlatImageHeader {
    pub fn requires_stack_guard(&self) -> bool {
        self.format_flags & flat_image_policy::REQUIRE_STACK_GUARD != 0
    }

    pub fn uses_segment_table(&self) -> bool {
        self.format_flags & flat_image_policy::SEGMENT_TABLE != 0
    }

    pub fn dependencies(&self) -> &[FlatDependencyRecord] {
        &self.dependencies[..self.dependency_count]
    }

    pub fn segments(&self) -> &[FlatSegmentRecord] {
        &self.segments[..self.segment_count]
    }
}

/// A companion library image actually mapped into a task's address space by
/// the loader.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadedLibraryRecord {
    /// BootStore image id of the mapped companion library.
    pub image_id: u32,
    pub base: VirtualAddress,
    pub mapped_bytes: usize,
}

impl LoadedLibraryRecord {
    pub const EMPTY: Self = Self {
        image_id: 0,
        base: VirtualAddress::new(0),
        mapped_bytes: 0,
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadedUserImage {
    pub entry_point: VirtualAddress,
    pub image_base: VirtualAddress,
    pub file_size: usize,
    pub mapped_image_bytes: usize,
    pub user_stack_top: VirtualAddress,
    pub mapped_stack_bytes: usize,
    /// True when the image explicitly requested an executable stack
    /// (ELF `PT_GNU_STACK` with the execute flag set).
    pub stack_executable: bool,
    pub libraries: [LoadedLibraryRecord; MAX_FLAT_DEPENDENCIES],
    pub library_count: usize,
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
    UnsupportedFormat,
    UnsupportedAbi,
    UnsupportedHeader,
    UnsupportedMachine,
    /// Image carries dynamic relocations this loader cannot apply (a
    /// non-`R_X86_64_RELATIVE` type or a relocation target outside the mapped
    /// image).
    UnsupportedRelocation,
    AddressAlignment,
    FrameExhausted,
    /// Image requires a newer kernel ABI than this loader provides.
    KernelAbiTooNew,
    /// A declared dependency image could not be resolved from the boot store.
    DependencyUnavailable,
    /// A resolved dependency image failed validation.
    DependencyInvalid,
    /// A strong undefined symbol had no definition across the loaded main
    /// image and its dependencies. `name` holds up to 32 bytes of the symbol
    /// name; `len` is the number of valid bytes in `name`.
    UnresolvedSymbol {
        name: [u8; 32],
        len: u8,
    },
    /// The load's shared symbol namespace ran out of entries while
    /// registering dependency exports.
    SymbolSpaceExhausted,
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
