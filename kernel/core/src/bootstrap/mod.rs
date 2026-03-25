use crate::memory::PhysicalAddress;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootMemoryRegion {
    pub start: PhysicalAddress,
    pub end: PhysicalAddress,
    pub kind: BootMemoryRegionKind,
}

impl BootMemoryRegion {
    pub const EMPTY: Self = Self {
        start: PhysicalAddress::new(0),
        end: PhysicalAddress::new(0),
        kind: BootMemoryRegionKind::Reserved,
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootMemoryRegionKind {
    Usable,
    BootServicesReclaimable,
    BootloaderOwned,
    AcpiReclaimable,
    FirmwareReserved,
    Device,
    Reserved,
    Unknown(u32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FramebufferInfo {
    pub physical_base: PhysicalAddress,
    pub byte_len: usize,
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub bytes_per_pixel: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootContext<'boot> {
    pub memory_regions: &'boot [BootMemoryRegion],
    pub memory_map_available: bool,
    pub memory_map_truncated: bool,
    pub physical_memory_offset: Option<u64>,
    pub rsdp_address: Option<PhysicalAddress>,
    pub framebuffer: Option<FramebufferInfo>,
}

impl<'boot> BootContext<'boot> {
    pub const fn memory_region_count(&self) -> usize {
        self.memory_regions.len()
    }

    pub fn usable_memory_region_count(&self) -> usize {
        self.memory_regions
            .iter()
            .filter(|region| matches!(region.kind, BootMemoryRegionKind::Usable))
            .count()
    }

    pub fn boot_services_reclaimable_region_count(&self) -> usize {
        self.memory_regions
            .iter()
            .filter(|region| matches!(region.kind, BootMemoryRegionKind::BootServicesReclaimable))
            .count()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootstrapStage {
    EarlyArchitectureBringUp,
    MemoryDiscovery,
    KernelObjectFoundation,
    RootTaskPreparation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootstrapPlan {
    pub current: BootstrapStage,
    pub next: Option<BootstrapStage>,
}

impl BootstrapPlan {
    pub const fn phase0() -> Self {
        Self {
            current: BootstrapStage::EarlyArchitectureBringUp,
            next: Some(BootstrapStage::MemoryDiscovery),
        }
    }

    pub const fn phase1() -> Self {
        Self {
            current: BootstrapStage::MemoryDiscovery,
            next: Some(BootstrapStage::KernelObjectFoundation),
        }
    }
}
