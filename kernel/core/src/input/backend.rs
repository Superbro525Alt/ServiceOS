use alloc::sync::Arc;
use serviceos_abi::{InputEventInfo, InputSourceInfo};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputSourceError {
    QueueEmpty,
    Busy,
    Unsupported,
}

pub trait InputBackend: Send + Sync {
    fn info(&self) -> InputSourceInfo;
    fn receive(&self) -> Result<InputEventInfo, InputSourceError>;
    fn poll(&self) -> bool;
}

pub struct InputSourceObject {
    backend: Arc<dyn InputBackend>,
}

impl InputSourceObject {
    pub fn new(backend: Arc<dyn InputBackend>) -> Self {
        Self { backend }
    }

    pub fn info(&self) -> InputSourceInfo {
        self.backend.info()
    }

    pub fn receive(&self) -> Result<InputEventInfo, InputSourceError> {
        match self.backend.receive() {
            Ok(event) => Ok(event),
            Err(InputSourceError::QueueEmpty) => {
                let _ = self.backend.poll();
                self.backend.receive()
            }
            Err(error) => Err(error),
        }
    }

    pub fn backend(&self) -> Arc<dyn InputBackend> {
        Arc::clone(&self.backend)
    }
}
