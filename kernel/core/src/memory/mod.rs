mod address_space;
mod heap;
mod layout;
mod phys;

pub use address_space::{AddressSpaceRoot, FutureUserAddressSpaceLayout, KernelAddressSpace};
pub use heap::HeapInfo;
pub use layout::KernelVirtualLayout;
pub use phys::{EarlyFrameAllocator, FrameAllocatorStats};

use crate::bootstrap::{BootContext, BootMemoryRegionKind};
use core::ops::{BitOr, BitOrAssign};
use spin::{Mutex, Once};

pub const PAGE_SIZE_BYTES: u64 = 4096;

static MEMORY_MANAGER: Once<MemoryManager> = Once::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct PhysicalAddress(u64);

impl PhysicalAddress {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn as_u64(self) -> u64 {
        self.0
    }

    pub const fn align_up(self, align: u64) -> Self {
        let remainder = self.0 % align;
        if remainder == 0 {
            self
        } else {
            Self(self.0 + (align - remainder))
        }
    }

    pub const fn align_down(self, align: u64) -> Self {
        Self(self.0 - (self.0 % align))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct VirtualAddress(u64);

impl VirtualAddress {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn as_u64(self) -> u64 {
        self.0
    }

    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }

    pub const fn offset(self, bytes: u64) -> Self {
        Self(self.0 + bytes)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageSize {
    Size4KiB,
    Size2MiB,
    Size1GiB,
}

impl PageSize {
    pub const fn bytes(self) -> u64 {
        match self {
            Self::Size4KiB => 4 * 1024,
            Self::Size2MiB => 2 * 1024 * 1024,
            Self::Size1GiB => 1024 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryUse {
    KernelText,
    KernelData,
    KernelStack,
    KernelHeap,
    KernelObjects,
    UserPrivate,
    Shared,
    DeviceMmio,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicalMemoryRange {
    pub start: PhysicalAddress,
    pub end: PhysicalAddress,
}

impl PhysicalMemoryRange {
    pub const fn size_bytes(self) -> u64 {
        self.end.as_u64() - self.start.as_u64()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VirtualMemoryRange {
    pub start: VirtualAddress,
    pub end: VirtualAddress,
    pub use_class: MemoryUse,
}

impl VirtualMemoryRange {
    pub const fn size_bytes(self) -> u64 {
        self.end.as_u64() - self.start.as_u64()
    }

    pub const fn page_count_4kib(self) -> usize {
        (self.size_bytes() / PAGE_SIZE_BYTES) as usize
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Frame {
    pub base: PhysicalAddress,
    pub size: PageSize,
}

impl Frame {
    pub const fn size_bytes(self) -> u64 {
        self.size.bytes()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MappingFlags(u8);

impl MappingFlags {
    pub const WRITABLE: Self = Self(1 << 0);
    pub const EXECUTABLE: Self = Self(1 << 1);
    pub const USER_ACCESSIBLE: Self = Self(1 << 2);
    pub const GLOBAL: Self = Self(1 << 3);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn kernel_data() -> Self {
        Self(Self::WRITABLE.0 | Self::GLOBAL.0)
    }
}

impl BitOr for MappingFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for MappingFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

pub trait PageMapper {
    fn active_root_frame(&self) -> PhysicalAddress;
    fn map_page(
        &mut self,
        page_start: VirtualAddress,
        frame: Frame,
        flags: MappingFlags,
        allocator: &mut EarlyFrameAllocator,
    ) -> Result<(), MappingError>;
    fn translate(&self, address: VirtualAddress) -> Option<PhysicalAddress>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MappingError {
    AddressAlignment,
    AlreadyMapped,
    FrameAllocationFailed,
    UnsupportedInPhase1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitializationError {
    MissingMemoryMap,
    HeapAlreadyInitialized,
    KernelHeapExhausted,
    Mapping(MappingError),
    TooManyUsableRegions,
}

impl From<MappingError> for InitializationError {
    fn from(error: MappingError) -> Self {
        Self::Mapping(error)
    }
}

pub struct MemoryManager {
    frame_allocator: Mutex<EarlyFrameAllocator>,
    stats: MemoryStats,
    kernel_address_space: KernelAddressSpace,
    heap: HeapInfo,
}

impl MemoryManager {
    pub fn frame_allocator(&self) -> &Mutex<EarlyFrameAllocator> {
        &self.frame_allocator
    }

    pub fn stats(&self) -> MemoryStats {
        self.stats
    }

    pub fn kernel_address_space(&self) -> KernelAddressSpace {
        self.kernel_address_space
    }

    pub fn heap(&self) -> HeapInfo {
        self.heap
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryStats {
    pub total_bytes: u64,
    pub usable_bytes: u64,
    pub boot_services_reclaimable_bytes: u64,
    pub acpi_reclaimable_bytes: u64,
    pub reserved_bytes: u64,
    pub mapped_heap_bytes: u64,
    pub remaining_usable_bytes: u64,
}

impl MemoryStats {
    fn from_boot_context(
        boot_context: &BootContext<'_>,
        heap: HeapInfo,
        frame_allocator: &EarlyFrameAllocator,
    ) -> Self {
        let mut total_bytes = 0u64;
        let mut usable_bytes = 0u64;
        let mut boot_services_reclaimable_bytes = 0u64;
        let mut acpi_reclaimable_bytes = 0u64;
        let mut reserved_bytes = 0u64;

        for region in boot_context.memory_regions {
            let bytes = region.end.as_u64() - region.start.as_u64();
            total_bytes += bytes;

            match region.kind {
                BootMemoryRegionKind::Usable => usable_bytes += bytes,
                BootMemoryRegionKind::BootServicesReclaimable => {
                    boot_services_reclaimable_bytes += bytes;
                    reserved_bytes += bytes;
                }
                BootMemoryRegionKind::AcpiReclaimable => {
                    acpi_reclaimable_bytes += bytes;
                    reserved_bytes += bytes;
                }
                _ => reserved_bytes += bytes,
            }
        }

        Self {
            total_bytes,
            usable_bytes,
            boot_services_reclaimable_bytes,
            acpi_reclaimable_bytes,
            reserved_bytes,
            mapped_heap_bytes: heap.range.size_bytes(),
            remaining_usable_bytes: frame_allocator.remaining_frames() * PAGE_SIZE_BYTES,
        }
    }
}

pub fn initialize(
    boot_context: &BootContext<'_>,
    mapper: &mut impl PageMapper,
) -> Result<&'static MemoryManager, InitializationError> {
    if !boot_context.memory_map_available {
        return Err(InitializationError::MissingMemoryMap);
    }

    let mut frame_allocator = EarlyFrameAllocator::from_boot_context(boot_context)?;
    let virtual_layout = KernelVirtualLayout::phase1_default();
    let heap = heap::initialize_kernel_heap(mapper, &mut frame_allocator, &virtual_layout)?;
    let kernel_address_space = KernelAddressSpace::new(
        AddressSpaceRoot::new(mapper.active_root_frame()),
        virtual_layout,
    );
    let stats = MemoryStats::from_boot_context(boot_context, heap, &frame_allocator);

    Ok(MEMORY_MANAGER.call_once(|| MemoryManager {
        frame_allocator: Mutex::new(frame_allocator),
        stats,
        kernel_address_space,
        heap,
    }))
}

pub fn manager() -> Option<&'static MemoryManager> {
    MEMORY_MANAGER.get()
}
