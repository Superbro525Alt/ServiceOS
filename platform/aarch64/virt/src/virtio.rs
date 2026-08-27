use alloc::vec::Vec;
use core::ptr::NonNull;

use serviceos_kernel_core::memory::{self, PhysicalAddress};
use virtio_drivers::{
    BufferDirection, Hal, PAGE_SIZE,
    transport::{DeviceType, Transport, mmio::MmioTransport},
};

#[cfg(target_arch = "aarch64")]
use serviceos_kernel_arch_aarch64::mmu::{self, OwnedPageTable};
#[cfg(target_arch = "aarch64")]
use serviceos_kernel_core::memory::{PageMapper, VirtualAddress};

use crate::dtb::VirtioMmioDevice;

extern crate alloc;

pub type VirtioTransport = MmioTransport<'static>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiscoveredVirtioDevice {
    pub mmio_base: u64,
    pub mmio_size: usize,
    pub irq: u32,
    pub device_type: DeviceType,
}

pub fn discover(
    devices: &[VirtioMmioDevice],
    wanted: DeviceType,
) -> Vec<(DiscoveredVirtioDevice, VirtioTransport)> {
    let mut found = Vec::new();
    for device in devices
        .iter()
        .copied()
        .filter(|device| device.is_populated())
    {
        let Some(header) = NonNull::new(device.base.as_u64() as *mut _) else {
            continue;
        };
        // SAFETY: the region was described by the device tree as a virtio-mmio
        // slot, is identity mapped by the kernel page tables, and stays valid
        // for the lifetime of the kernel.
        let transport = unsafe { VirtioTransport::new(header, device.size) };
        let Ok(transport) = transport else {
            continue;
        };
        if Transport::device_type(&transport) != wanted {
            // Probing must not reset the device: dropping an `MmioTransport`
            // writes STATUS=0 (a full device reset), which would wipe the
            // vring and status of devices already initialized by earlier
            // backends. Probe transports own no resources, so release them
            // without running the reset.
            core::mem::forget(transport);
            continue;
        }
        found.push((
            DiscoveredVirtioDevice {
                mmio_base: device.base.as_u64(),
                mmio_size: device.size,
                irq: device.irq,
                device_type: Transport::device_type(&transport),
            },
            transport,
        ));
    }
    found
}

pub struct KernelHal;

unsafe impl Hal for KernelHal {
    fn dma_alloc(pages: usize, _direction: BufferDirection) -> (u64, NonNull<u8>) {
        let Some(memory) = memory::manager() else {
            return (0, NonNull::dangling());
        };
        let mut allocator = memory.frame_allocator().lock();
        let Some(first) = allocator.allocate_4kib() else {
            return (0, NonNull::dangling());
        };
        let base = first.base.as_u64();
        unsafe {
            core::ptr::write_bytes(base as *mut u8, 0, PAGE_SIZE);
        }

        for page in 1..pages {
            let Some(next) = allocator.allocate_4kib() else {
                return (0, NonNull::dangling());
            };
            if next.base.as_u64() != base + (page as u64 * PAGE_SIZE as u64) {
                return (0, NonNull::dangling());
            }
            unsafe {
                core::ptr::write_bytes(next.base.as_u64() as *mut u8, 0, PAGE_SIZE);
            }
        }

        (
            base,
            NonNull::new(base as *mut u8).unwrap_or(NonNull::dangling()),
        )
    }

    unsafe fn dma_dealloc(_paddr: u64, _vaddr: NonNull<u8>, _pages: usize) -> i32 {
        0
    }

    unsafe fn mmio_phys_to_virt(paddr: u64, _size: usize) -> NonNull<u8> {
        NonNull::new(paddr as *mut u8).unwrap_or(NonNull::dangling())
    }

    unsafe fn share(buffer: NonNull<[u8]>, _direction: BufferDirection) -> u64 {
        translate_kernel_pointer(buffer.as_ptr().cast::<u8>() as u64)
            .map(PhysicalAddress::as_u64)
            .unwrap_or(0)
    }

    unsafe fn unshare(_paddr: u64, _buffer: NonNull<[u8]>, _direction: BufferDirection) {}
}

#[cfg(target_arch = "aarch64")]
fn translate_kernel_pointer(virtual_address: u64) -> Option<PhysicalAddress> {
    let mapper = unsafe { OwnedPageTable::from_root(mmu::current_page_table_root()) };
    mapper.translate(VirtualAddress::new(virtual_address))
}

#[cfg(not(target_arch = "aarch64"))]
fn translate_kernel_pointer(virtual_address: u64) -> Option<PhysicalAddress> {
    Some(PhysicalAddress::new(virtual_address))
}
