use alloc::sync::Arc;
use serviceos_abi::BlockDeviceInfo;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockDeviceError {
    InvalidOffset,
    BufferSize,
    Busy,
    Unsupported,
    Denied,
}

pub trait BlockBackend: Send + Sync {
    fn info(&self) -> BlockDeviceInfo;
    fn read_blocks(&self, start_block: u64, buffer: &mut [u8]) -> Result<usize, BlockDeviceError>;
    fn write_blocks(&self, start_block: u64, buffer: &[u8]) -> Result<usize, BlockDeviceError>;
}

pub struct BlockDeviceObject {
    backend: Arc<dyn BlockBackend>,
}

impl BlockDeviceObject {
    pub fn new(backend: Arc<dyn BlockBackend>) -> Self {
        Self { backend }
    }

    pub fn info(&self) -> BlockDeviceInfo {
        self.backend.info()
    }

    pub fn read_blocks(
        &self,
        start_block: u64,
        buffer: &mut [u8],
    ) -> Result<usize, BlockDeviceError> {
        self.backend.read_blocks(start_block, buffer)
    }

    pub fn write_blocks(&self, start_block: u64, buffer: &[u8]) -> Result<usize, BlockDeviceError> {
        self.backend.write_blocks(start_block, buffer)
    }

    pub fn backend(&self) -> Arc<dyn BlockBackend> {
        Arc::clone(&self.backend)
    }
}
