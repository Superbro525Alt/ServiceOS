use core::cell::UnsafeCell;

use serviceos_kernel_core::{
    bootstrap::{BootInfo, BootMemoryRegion, BootMemoryRegionKind},
    memory::{PAGE_SIZE_BYTES, PhysicalAddress},
};

use crate::{
    dtb::{self, DeviceTreeBootInfo, DeviceTreeError, InterruptControllerRegions},
    uart::UartDescriptor,
};

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
    let reserved_end = kernel_end.align_up(PAGE_SIZE_BYTES).as_u64().max(
        (dtb.dtb_base.as_u64() + dtb.dtb_size as u64).div_ceil(PAGE_SIZE_BYTES) * PAGE_SIZE_BYTES,
    );

    for range in dtb
        .memory_ranges
        .iter()
        .copied()
        .take(dtb.memory_range_count)
    {
        let start = range.start.as_u64();
        let end = range.end.as_u64();
        if start >= end {
            continue;
        }

        if start < reserved_end {
            let reserved_region_end = end.min(reserved_end);
            push_region(
                storage,
                &mut count,
                BootMemoryRegion {
                    start: PhysicalAddress::new(start),
                    end: PhysicalAddress::new(reserved_region_end),
                    kind: BootMemoryRegionKind::BootloaderOwned,
                },
            )?;
            if reserved_region_end < end {
                push_region(
                    storage,
                    &mut count,
                    BootMemoryRegion {
                        start: PhysicalAddress::new(reserved_region_end),
                        end: PhysicalAddress::new(end),
                        kind: BootMemoryRegionKind::Usable,
                    },
                )?;
            }
        } else {
            push_region(
                storage,
                &mut count,
                BootMemoryRegion {
                    start: PhysicalAddress::new(start),
                    end: PhysicalAddress::new(end),
                    kind: BootMemoryRegionKind::Usable,
                },
            )?;
        }
    }

    if kernel_start.as_u64() > 0 && count == 0 {
        return Err(BootError::TooManyMemoryRegions);
    }

    Ok(count)
}

fn push_region(
    storage: &mut [BootMemoryRegion],
    count: &mut usize,
    region: BootMemoryRegion,
) -> Result<(), BootError> {
    let Some(slot) = storage.get_mut(*count) else {
        return Err(BootError::TooManyMemoryRegions);
    };
    *slot = region;
    *count += 1;
    Ok(())
}
