use spin::{Mutex, Once};

use crate::bootstrap::{BootContext, BootMemoryRegionKind};

use super::{
    AddressSpaceRoot, EarlyFrameAllocator, Frame, HeapInfo, KernelAddressSpace,
    KernelVirtualLayout, MappingError, PAGE_SIZE_BYTES, PageMapper,
    oom::{self, OomError},
    pressure::{self, PressureLevel, PressureReading, PressureTransition},
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

    /// Live pressure reading across the two tracked domains: usable-frame
    /// headroom and kernel-heap headroom.
    pub fn pressure_reading(&self) -> PressureReading {
        let total_frames = self.stats.usable_bytes / PAGE_SIZE_BYTES;
        let free_frames = self.frame_allocator().lock().usable_headroom_frames();
        let (heap_total, heap_free) = super::heap::kernel_heap_usage()
            .map(|usage| (usage.total_bytes, usage.free_bytes))
            .unwrap_or((0, 0));

        PressureReading {
            frames_headroom_permille: pressure::headroom_permille(free_frames, total_frames),
            heap_headroom_permille: pressure::headroom_permille(heap_free, heap_total),
        }
    }

    /// Current classified pressure level, once pressure tracking started.
    pub fn current_pressure_level(&self) -> Option<PressureLevel> {
        pressure::current_level()
    }

    /// Reclassify from live headroom; returns the transition when the level
    /// changed (listeners notified by the pressure monitor itself).
    pub fn refresh_pressure(&self) -> Option<PressureTransition> {
        let tick = crate::time::manager().map_or(0, |manager| manager.now().0);
        pressure::observe(self.pressure_reading(), tick)
    }

    /// Frame allocation with the OOM policy applied on failure: reclaim one
    /// victim task's frames, then retry exactly once. Terminal exhaustion
    /// panics with an explicit message per the kernel OOM contract.
    pub fn allocate_frame_with_oom_policy(&self) -> Frame {
        if let Some(frame) = self.frame_allocator().lock().allocate_4kib() {
            return frame;
        }

        self.refresh_pressure();
        match oom::recover_with_retry(|| self.frame_allocator().lock().allocate_4kib()) {
            Ok(frame) => frame,
            Err(OomError::NoRecoveryAvailable) => panic!(
                "memory: OOM: frame allocation failed and no OOM recovery hooks are installed"
            ),
            Err(OomError::ProtectedSetExhausted) => panic!(
                "memory: OOM: frame allocation failed and no reclaimable victim was available (protected set exhausted)"
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryStats {
    pub total_bytes: u64,
    pub usable_bytes: u64,
    pub boot_services_reclaimable_bytes: u64,
    pub reclaimed_boot_services_bytes: u64,
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
        reclaimed_boot_services_bytes: u64,
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
            reclaimed_boot_services_bytes,
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
    let reclaimed_boot_services_bytes =
        frame_allocator.reclaim_boot_services(boot_context)? * PAGE_SIZE_BYTES;
    let kernel_address_space = KernelAddressSpace::new(
        AddressSpaceRoot::new(mapper.active_root_frame()),
        virtual_layout,
    );
    let stats = MemoryStats::from_boot_context(
        boot_context,
        heap,
        &frame_allocator,
        reclaimed_boot_services_bytes,
    );

    let memory_manager = MEMORY_MANAGER.call_once(|| MemoryManager {
        frame_allocator: Mutex::new(frame_allocator),
        stats,
        kernel_address_space,
        heap,
    });

    // Pressure tracking starts with a real reading so level transitions are
    // always measured against the boot baseline.
    pressure::initialize();
    memory_manager.refresh_pressure();

    Ok(memory_manager)
}

pub fn manager() -> Option<&'static MemoryManager> {
    MEMORY_MANAGER.get()
}
