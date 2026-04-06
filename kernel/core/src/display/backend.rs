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
    fn present_damage(
        &self,
        frame: &[u8],
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<(), DisplayOutputError> {
        let _ = (x, y, width, height);
        self.present(frame)
    }
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

    pub fn present_damage(
        &self,
        frame: &[u8],
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<(), DisplayOutputError> {
        self.backend.present_damage(frame, x, y, width, height)
    }
}
