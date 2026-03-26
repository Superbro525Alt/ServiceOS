use alloc::sync::Arc;
use serviceos_abi::DisplayOutputInfo;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayOutputError {
    BufferTooSmall,
    Busy,
    Unsupported,
}

pub trait DisplayBackend: Send + Sync {
    fn info(&self) -> DisplayOutputInfo;
    fn present(&self, frame: &[u8]) -> Result<(), DisplayOutputError>;
}

pub struct DisplayOutputObject {
    backend: Arc<dyn DisplayBackend>,
}

impl DisplayOutputObject {
    pub fn new(backend: Arc<dyn DisplayBackend>) -> Self {
        Self { backend }
    }

    pub fn info(&self) -> DisplayOutputInfo {
        self.backend.info()
    }

    pub fn present(&self, frame: &[u8]) -> Result<(), DisplayOutputError> {
        self.backend.present(frame)
    }
}
