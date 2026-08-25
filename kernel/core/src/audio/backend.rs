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

    /// Push interleaved s16le stereo frames (4 bytes per frame at the
    /// sink rate) to a PCM playback sink. Backends without a PCM path
    /// return `Unsupported`; byte counts must be non-zero multiples of 4.
    /// Returns the number of bytes accepted.
    fn pcm_write_s16le_stereo(&self, _bytes: &[u8]) -> Result<usize, AudioEndpointError> {
        Err(AudioEndpointError::Unsupported)
    }
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

    pub fn pcm_write_s16le_stereo(&self, bytes: &[u8]) -> Result<usize, AudioEndpointError> {
        self.backend.pcm_write_s16le_stereo(bytes)
    }
}
