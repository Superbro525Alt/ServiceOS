use spin::{Mutex, Once};

use crate::bootstrap::{BootContext, BootMemoryRegionKind};

use super::{
    AddressSpaceRoot, EarlyFrameAllocator, HeapInfo, KernelAddressSpace, KernelVirtualLayout,
    MappingError, PAGE_SIZE_BYTES, PageMapper,
};

static MEMORY_MANAGER: Once<MemoryManager> = Once::new();

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

pub fn initialize(
    boot_context: &BootContext<'_>,
    mapper: &mut impl PageMapper,
) -> Result<&'static MemoryManager, InitializationError> {
    if !boot_context.memory_map_available {
        return Err(InitializationError::MissingMemoryMap);
    }

    let mut frame_allocator = EarlyFrameAllocator::from_boot_context(boot_context)?;
    let virtual_layout = KernelVirtualLayout::bootstrap_default();
    let heap = super::heap::initialize_kernel_heap(mapper, &mut frame_allocator, &virtual_layout)?;
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
