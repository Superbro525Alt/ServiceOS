use alloc::sync::Arc;
use spin::{Mutex, Once};

use serviceos_abi::{BlockDeviceBackend, BlockDeviceInfo};
use serviceos_kernel_core::block::{BlockBackend, BlockDeviceError};
use virtio_drivers::{
    device::blk::{SECTOR_SIZE, VirtIOBlk},
    transport::DeviceType,
};

use crate::dtb::VirtioMmioDevice;
use crate::virtio::{KernelHal, VirtioTransport, discover};

const LEGACY_MMIO_DEVICE_TYPE: DeviceType = DeviceType::Block;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockBringupSummary {
    pub backend: BlockDeviceBackend,
    pub mmio_base: u64,
    pub irq: u32,
    pub writable: bool,
    pub block_size: u32,
    pub block_count: u64,
}

pub fn initialize(devices: &[VirtioMmioDevice]) -> Option<Arc<dyn BlockBackend>> {
    let (discovered, transport) = discover(devices, LEGACY_MMIO_DEVICE_TYPE)
        .into_iter()
        .next()?;
    let device = VirtIOBlk::<KernelHal, VirtioTransport>::new(transport).ok()?;
    let summary = BlockBringupSummary {
        backend: BlockDeviceBackend::VirtioPci,
        mmio_base: discovered.mmio_base,
        irq: discovered.irq,
        writable: !device.readonly(),
        block_size: SECTOR_SIZE as u32,
        block_count: device.capacity(),
    };
    let backend = Arc::new(VirtioBlockBackend::new(device));
    let _ = BRINGUP_SUMMARY.call_once(|| summary);
    Some(backend)
}

pub fn bringup_summary() -> Option<BlockBringupSummary> {
    BRINGUP_SUMMARY.get().copied()
}

static BRINGUP_SUMMARY: Once<BlockBringupSummary> = Once::new();

struct VirtioBlockBackend {
    state: Mutex<VirtioBlockState>,
}

struct VirtioBlockState {
    device: VirtIOBlk<KernelHal, VirtioTransport>,
    writable: bool,
    block_size: usize,
    block_count: u64,
    read_ops: u64,
    write_ops: u64,
}

impl VirtioBlockBackend {
    fn new(device: VirtIOBlk<KernelHal, VirtioTransport>) -> Self {
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
