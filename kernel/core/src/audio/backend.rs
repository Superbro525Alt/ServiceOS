use alloc::sync::Arc;
use serviceos_abi::{AudioEndpointInfo, AudioToneRequest};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioEndpointError {
    Busy,
    Unsupported,
}

pub trait AudioBackend: Send + Sync {
    fn info(&self) -> AudioEndpointInfo;
    fn play_tone(&self, request: AudioToneRequest) -> Result<(), AudioEndpointError>;
    fn stop(&self) -> Result<(), AudioEndpointError>;
}

pub struct AudioEndpointObject {
    backend: Arc<dyn AudioBackend>,
}

impl AudioEndpointObject {
    pub fn new(backend: Arc<dyn AudioBackend>) -> Self {
        Self { backend }
    }

    pub fn info(&self) -> AudioEndpointInfo {
        self.backend.info()
    }

    pub fn play_tone(&self, request: AudioToneRequest) -> Result<(), AudioEndpointError> {
        self.backend.play_tone(request)
    }

    pub fn stop(&self) -> Result<(), AudioEndpointError> {
        self.backend.stop()
    }
}
