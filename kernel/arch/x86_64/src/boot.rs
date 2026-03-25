use serviceos_kernel_core::bootstrap::{BootContext, BootMemoryRegion};

const EMPTY_MEMORY_MAP: [BootMemoryRegion; 0] = [];

pub fn boot_context_from_uefi() -> BootContext<'static> {
    BootContext {
        memory_regions: &EMPTY_MEMORY_MAP,
        memory_map_available: false,
        memory_map_truncated: false,
        physical_memory_offset: None,
        rsdp_address: None,
        framebuffer: None,
    }
}
