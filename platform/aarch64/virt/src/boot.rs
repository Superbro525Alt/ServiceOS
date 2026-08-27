use core::cell::UnsafeCell;

use serviceos_kernel_core::{
    bootstrap::{BootInfo, BootMemoryRegion, BootMemoryRegionKind},
    memory::{PAGE_SIZE_BYTES, PhysicalAddress},
};

use crate::dtb::{
    self, DeviceTreeBootInfo, DeviceTreeError, InterruptControllerRegions, MAX_VIRTIO_MMIO_DEVICES,
    VirtioMmioDevice,
};
use crate::uart::UartDescriptor;

const MAX_BOOT_MEMORY_REGIONS: usize = 32;

struct BootMemoryMapBuffer(UnsafeCell<[BootMemoryRegion; MAX_BOOT_MEMORY_REGIONS]>);

unsafe impl Sync for BootMemoryMapBuffer {}

static BOOT_MEMORY_MAP: BootMemoryMapBuffer = BootMemoryMapBuffer(UnsafeCell::new(
    [BootMemoryRegion::EMPTY; MAX_BOOT_MEMORY_REGIONS],
));

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootSupportStatus {
    pub boot_info_ready: bool,
    pub dtb_parsing: bool,
    pub serial_console: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootError {
    DeviceTree(DeviceTreeError),
    TooManyMemoryRegions,
}

impl From<DeviceTreeError> for BootError {
    fn from(error: DeviceTreeError) -> Self {
        Self::DeviceTree(error)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BootSummary<'boot> {
    pub model: &'boot str,
    pub compatible: Option<&'boot str>,
    pub serial_number: Option<&'boot str>,
    pub dtb_base: PhysicalAddress,
    pub dtb_size: usize,
    pub uart: Option<UartDescriptor<'boot>>,
    pub interrupt_controller: Option<InterruptControllerRegions>,
    pub timer_ppi_intid: Option<u16>,
    pub virtio_mmio_devices: [VirtioMmioDevice; MAX_VIRTIO_MMIO_DEVICES],
    pub virtio_mmio_count: usize,
}

pub struct CapturedBootState<'boot> {
    pub boot_info: BootInfo<'boot>,
    pub summary: BootSummary<'boot>,
}

pub fn boot_support_status() -> BootSupportStatus {
    BootSupportStatus {
        boot_info_ready: true,
        dtb_parsing: true,
        serial_console: true,
    }
}

pub fn capture_boot_info(
    dtb_ptr: *const u8,
    kernel_start: PhysicalAddress,
    kernel_end: PhysicalAddress,
) -> Result<CapturedBootState<'static>, BootError> {
    let dtb = dtb::parse(dtb_ptr)?;
    let storage = unsafe { &mut *BOOT_MEMORY_MAP.0.get() };
    let region_count = build_boot_memory_map(storage, &dtb, kernel_start, kernel_end)?;

    Ok(CapturedBootState {
        boot_info: BootInfo {
            memory_regions: &storage[..region_count],
            memory_map_available: true,
            memory_map_truncated: dtb.memory_map_truncated,
            physical_memory_offset: None,
            rsdp_address: None,
            framebuffer: None,
            boot_store: None,
        },
        summary: BootSummary {
            model: dtb.model,
            compatible: dtb.compatible,
            serial_number: dtb.serial_number,
            dtb_base: dtb.dtb_base,
            dtb_size: dtb.dtb_size,
            uart: dtb.stdout_uart,
            interrupt_controller: dtb.interrupt_controller,
            timer_ppi_intid: dtb.timer_ppi_intid,
            virtio_mmio_devices: dtb.virtio_mmio_devices,
            virtio_mmio_count: dtb.virtio_mmio_count,
        },
    })
}

fn build_boot_memory_map(
    storage: &mut [BootMemoryRegion],
    dtb: &DeviceTreeBootInfo<'_>,
    kernel_start: PhysicalAddress,
    kernel_end: PhysicalAddress,
) -> Result<usize, BootError> {
    let mut count = 0usize;
    let mut reserved = [
        ReservedRange {
            start: align_down_page(kernel_start.as_u64()),
            end: align_up_page(kernel_end.as_u64()),
        },
        ReservedRange {
            start: align_down_page(dtb.dtb_base.as_u64()),
            end: align_up_page(dtb.dtb_base.as_u64().saturating_add(dtb.dtb_size as u64)),
        },
    ];
    reserved.sort_by_key(|range| range.start);

    for range in dtb
        .memory_ranges
        .iter()
        .copied()
        .take(dtb.memory_range_count)
    {
        let range_start = range.start.as_u64();
        let range_end = range.end.as_u64();
        if range_start >= range_end {
            continue;
        }

        let mut cursor = range_start;
        let mut previous_reserved_end = 0u64;
        for reservation in &reserved {
            let start = reservation.start.max(range_start);
            let end = reservation.end.min(range_end);
            if start >= end {
                continue;
            }
            if cursor < start {
                push_region(
                    storage,
                    &mut count,
                    BootMemoryRegion {
                        start: PhysicalAddress::new(cursor),
                        end: PhysicalAddress::new(start),
                        kind: BootMemoryRegionKind::Usable,
                    },
                )?;
                cursor = start;
            }
            let merged_start = start.max(cursor);
            let merged_end = end.max(cursor);
            let extends_previous = count > 0
                && storage[count - 1].kind == BootMemoryRegionKind::BootloaderOwned
                && storage[count - 1].end.as_u64() == merged_start
                && previous_reserved_end >= merged_start;
            if extends_previous {
                storage[count - 1].end = PhysicalAddress::new(merged_end);
            } else if merged_end > merged_start {
                push_region(
                    storage,
                    &mut count,
                    BootMemoryRegion {
                        start: PhysicalAddress::new(merged_start),
                        end: PhysicalAddress::new(merged_end),
                        kind: BootMemoryRegionKind::BootloaderOwned,
                    },
                )?;
            }
            cursor = cursor.max(merged_end);
            previous_reserved_end = reservation.end;
        }
        if cursor < range_end {
            push_region(
                storage,
                &mut count,
                BootMemoryRegion {
                    start: PhysicalAddress::new(cursor),
                    end: PhysicalAddress::new(range_end),
                    kind: BootMemoryRegionKind::Usable,
                },
            )?;
        }
    }

    if count == 0 {
        return Err(BootError::TooManyMemoryRegions);
    }

    Ok(count)
}

struct ReservedRange {
    start: u64,
    end: u64,
}

fn align_down_page(address: u64) -> u64 {
    address - (address % PAGE_SIZE_BYTES)
}

fn align_up_page(address: u64) -> u64 {
    address.div_ceil(PAGE_SIZE_BYTES) * PAGE_SIZE_BYTES
}

fn push_region(
    storage: &mut [BootMemoryRegion],
    count: &mut usize,
    region: BootMemoryRegion,
) -> Result<(), BootError> {
    if region.end.as_u64() <= region.start.as_u64() {
        return Ok(());
    }
    let Some(slot) = storage.get_mut(*count) else {
        return Err(BootError::TooManyMemoryRegions);
    };
    *slot = region;
    *count += 1;
    Ok(())
}
