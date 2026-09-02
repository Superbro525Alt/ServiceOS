//! virtio-rng entropy probe: binds the QEMU virtio-rng-pci device to the
//! kernel's `EntropySource` contract so the kernel DRBG seeds from real
//! hardware entropy at boot. Polled like the block device's control queue
//! (the driver waits on the queue, no IRQ handler is registered); a boot
//! without the device simply yields `None` and the kernel falls back to
//! jitter-only conditioning.

use alloc::sync::Arc;
use core::ptr::NonNull;

use serviceos_kernel_arch_x86_64::paging::ActivePageTable;
use serviceos_kernel_core::{
    memory::{self, PageMapper, PhysicalAddress, VirtualAddress},
    rng::EntropySource,
};
use spin::Mutex;
use virtio_drivers::{
    BufferDirection, Hal, PAGE_SIZE,
    device::rng::VirtIORng,
    transport::{
        DeviceType,
        pci::{
            PciTransport,
            bus::{Command, HeaderType, PciRoot},
            virtio_device_type,
        },
    },
};

use crate::msix::IoPortPciConfigAccess;

struct KernelHal;

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

fn translate_kernel_pointer(virtual_address: u64) -> Option<PhysicalAddress> {
    let mapper = unsafe { ActivePageTable::new_identity_mapped() };
    mapper.translate(VirtualAddress::new(virtual_address))
}

/// Kernel-facing wrapper around the polled virtio-rng device.
struct VirtioRngSource {
    device: Mutex<VirtIORng<KernelHal, PciTransport>>,
}

impl EntropySource for VirtioRngSource {
    fn request_entropy(&self, dst: &mut [u8]) -> Option<usize> {
        let mut device = self.device.lock();
        device.request_entropy(dst).ok()
    }
}

/// Probe the PCI bus for a virtio entropy-source device. Returns the kernel
/// entropy source on success; any failure (or a boot without the device)
/// returns None and the kernel seeds from jitter alone.
pub fn initialize() -> Option<Arc<dyn EntropySource>> {
    let mut root = PciRoot::new(IoPortPciConfigAccess);
    for bus in 0u16..=255 {
        for (device_function, info) in root.enumerate_bus(bus as u8) {
            if info.header_type != HeaderType::Standard {
                continue;
            }
            if virtio_device_type(&info) != Some(DeviceType::EntropySource) {
                continue;
            }

            let mut command = root.get_status_command(device_function).1;
            command.insert(Command::BUS_MASTER | Command::MEMORY_SPACE);
            root.set_command(device_function, command);

            let transport = PciTransport::new::<KernelHal, _>(&mut root, device_function).ok()?;
            let device = VirtIORng::<KernelHal, _>::new(transport).ok()?;
            return Some(Arc::new(VirtioRngSource {
                device: Mutex::new(device),
            }));
        }
    }
    None
}
