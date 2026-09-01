use alloc::sync::Arc;
use core::ptr::NonNull;

use serviceos_abi::{BlockDeviceBackend, BlockDeviceInfo};
use serviceos_kernel_arch_x86_64::{interrupts, paging::ActivePageTable};
use serviceos_kernel_core::{
    block::{BlockBackend, BlockDeviceError},
    memory::{self, PageMapper, PhysicalAddress, VirtualAddress},
};
use spin::{Mutex, Once};
use virtio_drivers::{
    BufferDirection, Hal, PAGE_SIZE,
    device::blk::{SECTOR_SIZE, VirtIOBlk},
    transport::pci::{
        PciTransport,
        bus::{Command, HeaderType, PciRoot},
        virtio_device_type,
    },
};

use crate::msix::{IoPortPciConfigAccess, MsixOutcome, MsixSteeringPlan, try_setup_msix};

/// Arch MSI vector slot the virtio block device owns (slot 0 is the NIC's).
const MSI_VECTOR_SLOT: u8 = 1;

/// How the block device's interrupts reach the kernel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockInterruptModel {
    /// Legacy INT#x pin emulation: the device keeps its default pin route
    /// (the driver polls completions, no external handler is registered).
    Legacy,
    /// MSI-X: message-signaled interrupt on an arch MSI vector, delivered
    /// through the LAPIC (no external controller involvement).
    Msix(u8),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockBringupSummary {
    pub backend: BlockDeviceBackend,
    pub pci_bus: u8,
    pub pci_device: u8,
    pub pci_function: u8,
    pub interrupt: BlockInterruptModel,
    pub writable: bool,
    pub block_size: u32,
    pub block_count: u64,
}

pub fn initialize() -> Option<Arc<dyn BlockBackend>> {
    let mut root = PciRoot::new(IoPortPciConfigAccess);
    for bus in 0u16..=255 {
        for (device_function, info) in root.enumerate_bus(bus as u8) {
            if info.header_type != HeaderType::Standard {
                continue;
            }
            if virtio_device_type(&info) != Some(virtio_drivers::transport::DeviceType::Block) {
                continue;
            }

            let mut command = root.get_status_command(device_function).1;
            command.insert(Command::BUS_MASTER | Command::MEMORY_SPACE);
            root.set_command(device_function, command);

            // Interrupt model: prefer MSI-X on the block device's own vector
            // (arch slot 1, after the NIC's slot 0) so completion signaling
            // leaves the shared legacy INT#x line. Same fallback discipline
            // as the NIC: every fallible MSI-X step happens before the device
            // is enabled, so a failure leaves the device untouched on its
            // default (legacy) route. The driver polls completions, so the
            // MSI-X handler only owns the vector (LAPIC EOI) — QEMU's
            // virtio-pci MSI-X path signals per event and does not gate
            // re-delivery on the ISR status register.
            let interrupt = match try_setup_msix(
                &mut root,
                device_function,
                // Block has a single queue and no post-init config reads the
                // driver cares about, so it stays one vector, config change
                // off (NO_VECTOR) — same as v0.
                MsixSteeringPlan {
                    queue_vector: interrupts::MSI_VECTOR_BASE + MSI_VECTOR_SLOT,
                    config_vector: None,
                },
            ) {
                MsixOutcome::Ready(steering) => {
                    if !interrupts::register_msi_vector_handler(MSI_VECTOR_SLOT, handle_block_irq) {
                        return None;
                    }
                    BlockInterruptModel::Msix(steering.queue_vector)
                }
                MsixOutcome::Disabled => BlockInterruptModel::Legacy,
                MsixOutcome::Failed(reason) => {
                    let _ = BLOCK_MSIX_SETUP_DIAG.call_once(|| reason);
                    BlockInterruptModel::Legacy
                }
            };

            let transport = PciTransport::new::<KernelHal, _>(&mut root, device_function).ok()?;
            let device = VirtIOBlk::<KernelHal, _>::new(transport).ok()?;
            let summary = BlockBringupSummary {
                backend: BlockDeviceBackend::VirtioPci,
                pci_bus: device_function.bus,
                pci_device: device_function.device,
                pci_function: device_function.function,
                interrupt,
                writable: !device.readonly(),
                block_size: SECTOR_SIZE as u32,
                block_count: device.capacity(),
            };
            let backend = Arc::new(VirtioBlockBackend::new(device));
            let _ = BRINGUP_SUMMARY.call_once(|| summary);
            return Some(backend);
        }
    }

    None
}

pub fn bringup_summary() -> Option<BlockBringupSummary> {
    BRINGUP_SUMMARY.get().copied()
}

static BRINGUP_SUMMARY: Once<BlockBringupSummary> = Once::new();

/// Diagnostics for the last skipped MSI-X bring-up (printed once by the
/// image's storage summary; temporary instrumentation).
pub static BLOCK_MSIX_SETUP_DIAG: Once<&'static str> = Once::new();

/// MSI-X completion deliveries for the polled block driver: the LAPIC EOI in
/// the dispatcher is the whole job (see the bring-up comment for why no
/// device-side ack is needed under MSI-X).
fn handle_block_irq(_slot: u8) {}

struct VirtioBlockBackend {
    state: Mutex<VirtioBlockState>,
}

struct VirtioBlockState {
    device: VirtIOBlk<KernelHal, PciTransport>,
    writable: bool,
    block_size: usize,
    block_count: u64,
    read_ops: u64,
    write_ops: u64,
}

impl VirtioBlockBackend {
    fn new(device: VirtIOBlk<KernelHal, PciTransport>) -> Self {
        let writable = !device.readonly();
        let block_count = device.capacity();
        Self {
            state: Mutex::new(VirtioBlockState {
                device,
                writable,
                block_size: SECTOR_SIZE,
                block_count,
                read_ops: 0,
                write_ops: 0,
            }),
        }
    }
}

impl BlockBackend for VirtioBlockBackend {
    fn info(&self) -> BlockDeviceInfo {
        let state = self.state.lock();
        BlockDeviceInfo {
            backend: BlockDeviceBackend::VirtioPci as u32,
            writable: u32::from(state.writable),
            block_size: state.block_size as u32,
            reserved: 0,
            block_count: state.block_count,
            read_ops: state.read_ops,
            write_ops: state.write_ops,
        }
    }

    fn read_blocks(&self, start_block: u64, buffer: &mut [u8]) -> Result<usize, BlockDeviceError> {
        let mut state = self.state.lock();
        if buffer.is_empty() || buffer.len() % state.block_size != 0 {
            return Err(BlockDeviceError::BufferSize);
        }
        let block_count = (buffer.len() / state.block_size) as u64;
        if start_block
            .checked_add(block_count)
            .is_none_or(|end| end > state.block_count)
        {
            return Err(BlockDeviceError::InvalidOffset);
        }
        state
            .device
            .read_blocks(start_block as usize, buffer)
            .map_err(|_| BlockDeviceError::Busy)?;
        state.read_ops = state.read_ops.saturating_add(1);
        Ok(buffer.len())
    }

    fn write_blocks(&self, start_block: u64, buffer: &[u8]) -> Result<usize, BlockDeviceError> {
        let mut state = self.state.lock();
        if !state.writable {
            return Err(BlockDeviceError::Denied);
        }
        if buffer.is_empty() || buffer.len() % state.block_size != 0 {
            return Err(BlockDeviceError::BufferSize);
        }
        let block_count = (buffer.len() / state.block_size) as u64;
        if start_block
            .checked_add(block_count)
            .is_none_or(|end| end > state.block_count)
        {
            return Err(BlockDeviceError::InvalidOffset);
        }
        state
            .device
            .write_blocks(start_block as usize, buffer)
            .map_err(|_| BlockDeviceError::Busy)?;
        let _ = state.device.flush();
        state.write_ops = state.write_ops.saturating_add(1);
        Ok(buffer.len())
    }
}

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
