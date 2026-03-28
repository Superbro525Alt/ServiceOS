use alloc::sync::Arc;
use serviceos_abi::PacketInterfaceInfo;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PacketInterfaceError {
    QueueEmpty,
    BufferTooSmall,
    Busy,
    Unsupported,
}

pub trait PacketBackend: Send + Sync {
    fn info(&self) -> PacketInterfaceInfo;
    fn transmit(&self, frame: &[u8]) -> Result<(), PacketInterfaceError>;
    fn receive(&self, buffer: &mut [u8]) -> Result<usize, PacketInterfaceError>;
    fn poll(&self) -> bool;
}

pub struct PacketInterfaceObject {
    backend: Arc<dyn PacketBackend>,
}

impl PacketInterfaceObject {
    pub fn new(backend: Arc<dyn PacketBackend>) -> Self {
        Self { backend }
    }

    pub fn info(&self) -> PacketInterfaceInfo {
        self.backend.info()
    }

    pub fn transmit(&self, frame: &[u8]) -> Result<(), PacketInterfaceError> {
        self.backend.transmit(frame)
    }

    pub fn receive(&self, buffer: &mut [u8]) -> Result<usize, PacketInterfaceError> {
        match self.backend.receive(buffer) {
            Ok(length) => Ok(length),
            Err(PacketInterfaceError::QueueEmpty) => {
                let _ = self.backend.poll();
                self.backend.receive(buffer)
            }
            Err(error) => Err(error),
        }
    }

    pub fn backend(&self) -> Arc<dyn PacketBackend> {
        Arc::clone(&self.backend)
    }
}
