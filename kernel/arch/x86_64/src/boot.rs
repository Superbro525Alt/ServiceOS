use core::cell::UnsafeCell;
use serviceos_kernel_core::{
    bootstrap::{BootContext, BootMemoryRegion, BootMemoryRegionKind},
    memory::PhysicalAddress,
};
use uefi::{
    boot::{self, PAGE_SIZE},
    mem::memory_map::{MemoryMap, MemoryType},
    system,
    table::cfg::{ACPI_GUID, ACPI2_GUID},
};

const MAX_BOOT_MEMORY_REGIONS: usize = 256;

struct BootMemoryMapBuffer(UnsafeCell<[BootMemoryRegion; MAX_BOOT_MEMORY_REGIONS]>);

unsafe impl Sync for BootMemoryMapBuffer {}

static BOOT_MEMORY_MAP: BootMemoryMapBuffer = BootMemoryMapBuffer(UnsafeCell::new(
    [BootMemoryRegion::EMPTY; MAX_BOOT_MEMORY_REGIONS],
));

pub fn exit_boot_services_and_capture_context() -> BootContext<'static> {
    let rsdp_address = system::with_config_table(|entries| {
        entries
            .iter()
            .find(|entry| entry.guid == ACPI2_GUID || entry.guid == ACPI_GUID)
            .map(|entry| PhysicalAddress::new(entry.address as u64))
    });

    let memory_map = unsafe { boot::exit_boot_services(MemoryType::LOADER_DATA) };
    let storage = unsafe { &mut *BOOT_MEMORY_MAP.0.get() };
    let mut count = 0usize;
    let mut truncated = false;

    for descriptor in memory_map.entries() {
        if count == storage.len() {
            truncated = true;
            break;
        }

        let start = PhysicalAddress::new(descriptor.phys_start);
        let end = PhysicalAddress::new(
            descriptor.phys_start + descriptor.page_count.saturating_mul(PAGE_SIZE as u64),
        );

        storage[count] = BootMemoryRegion {
            start,
            end,
            kind: normalize_memory_type(descriptor.ty),
        };
        count += 1;
    }

    BootContext {
        memory_regions: &storage[..count],
        memory_map_available: true,
        memory_map_truncated: truncated,
        physical_memory_offset: Some(0),
        rsdp_address,
        framebuffer: None,
    }
}

fn normalize_memory_type(memory_type: MemoryType) -> BootMemoryRegionKind {
    match memory_type {
        MemoryType::CONVENTIONAL => BootMemoryRegionKind::Usable,
        MemoryType::BOOT_SERVICES_CODE | MemoryType::BOOT_SERVICES_DATA => {
            BootMemoryRegionKind::BootServicesReclaimable
        }
        MemoryType::LOADER_CODE | MemoryType::LOADER_DATA => BootMemoryRegionKind::BootloaderOwned,
        MemoryType::ACPI_RECLAIM => BootMemoryRegionKind::AcpiReclaimable,
        MemoryType::ACPI_NON_VOLATILE
        | MemoryType::RUNTIME_SERVICES_CODE
        | MemoryType::RUNTIME_SERVICES_DATA => BootMemoryRegionKind::FirmwareReserved,
        MemoryType::MMIO | MemoryType::MMIO_PORT_SPACE => BootMemoryRegionKind::Device,
        MemoryType::RESERVED | MemoryType::UNUSABLE | MemoryType::PAL_CODE => {
            BootMemoryRegionKind::Reserved
        }
        _ => BootMemoryRegionKind::Unknown(memory_type.0),
    }
}
