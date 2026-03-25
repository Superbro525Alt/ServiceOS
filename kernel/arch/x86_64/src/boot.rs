use core::cell::UnsafeCell;
use serviceos_kernel_core::{
    bootstrap::{BootContext, BootMemoryRegion, BootMemoryRegionKind},
    memory::PhysicalAddress,
};
use uefi::{
    boot::{self, AllocateType, PAGE_SIZE},
    cstr16,
    mem::memory_map::{MemoryMap, MemoryType},
    proto::media::file::{File, FileAttribute, FileInfo, FileMode, FileType},
    system,
    table::cfg::{ACPI_GUID, ACPI2_GUID},
};

const MAX_BOOT_MEMORY_REGIONS: usize = 256;
const BOOT_STORE_PATH: &uefi::CStr16 = cstr16!(r"\serviceos\bootstore.bin");

struct BootMemoryMapBuffer(UnsafeCell<[BootMemoryRegion; MAX_BOOT_MEMORY_REGIONS]>);
struct BootStoreBuffer {
    ptr: UnsafeCell<*mut u8>,
    len: UnsafeCell<usize>,
}

unsafe impl Sync for BootMemoryMapBuffer {}
unsafe impl Sync for BootStoreBuffer {}

static BOOT_MEMORY_MAP: BootMemoryMapBuffer = BootMemoryMapBuffer(UnsafeCell::new(
    [BootMemoryRegion::EMPTY; MAX_BOOT_MEMORY_REGIONS],
));
static BOOT_STORE: BootStoreBuffer = BootStoreBuffer {
    ptr: UnsafeCell::new(core::ptr::null_mut()),
    len: UnsafeCell::new(0),
};

pub fn exit_boot_services_and_capture_context() -> BootContext<'static> {
    let boot_store = load_boot_store();
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
        boot_store,
    }
}

fn load_boot_store() -> Option<&'static [u8]> {
    let mut fs = boot::get_image_file_system(boot::image_handle()).ok()?;
    let mut volume = fs.open_volume().ok()?;
    let file = volume
        .open(BOOT_STORE_PATH, FileMode::Read, FileAttribute::empty())
        .ok()?;
    let mut file = match file.into_type().ok()? {
        FileType::Regular(file) => file,
        FileType::Dir(_) => return None,
    };
    let mut info_buf = [0u8; 512];
    let info = file.get_info::<FileInfo>(&mut info_buf).ok()?;
    let file_size = usize::try_from(info.file_size()).ok()?;
    if file_size == 0 {
        return Some(&[]);
    }

    let pages = file_size.div_ceil(PAGE_SIZE);
    let allocation =
        boot::allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, pages).ok()?;
    let buffer = unsafe { core::slice::from_raw_parts_mut(allocation.as_ptr(), pages * PAGE_SIZE) };
    let read_len = file.read(&mut buffer[..file_size]).ok()?;
    if read_len != file_size {
        return None;
    }

    unsafe {
        *BOOT_STORE.ptr.get() = allocation.as_ptr();
        *BOOT_STORE.len.get() = file_size;
        Some(core::slice::from_raw_parts(
            *BOOT_STORE.ptr.get(),
            *BOOT_STORE.len.get(),
        ))
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
