use rt::{AudioStreamDirection, AudioStreamState};
use serviceos_abi::AudioSampleFormat;
use serviceos_userspace_runtime as rt;

/// Service-side state of a null-capture stream: the negotiated format,
/// the pacing origin, and everything already synthesized for readers.
#[derive(Clone, Copy)]
pub(crate) struct CaptureStreamState {
    pub(crate) format: AudioSampleFormat,
    pub(crate) rate_hz: u32,
    pub(crate) channels: u32,
    pub(crate) start_tick: u64,
    pub(crate) frames_produced: u64,
    pub(crate) checksum: u64,
}

#[derive(Clone, Copy)]
pub(crate) struct StreamSlot {
    pub(crate) active: bool,
    pub(crate) control_handle: rt::Handle,
    pub(crate) session_id: u32,
    pub(crate) endpoint_index: u32,
    pub(crate) frequency_hz: u32,
    pub(crate) until_tick: u64,
    pub(crate) state: AudioStreamState,
    pub(crate) pcm_configured: bool,
    pub(crate) direction: AudioStreamDirection,
    pub(crate) capture: Option<CaptureStreamState>,
}

impl StreamSlot {
    pub(crate) const fn empty() -> Self {
        Self {
            active: false,
            control_handle: rt::INVALID_HANDLE,
            session_id: 0,
            endpoint_index: 0,
            frequency_hz: 0,
            until_tick: 0,
            state: AudioStreamState::Closed,
            pcm_configured: false,
            direction: AudioStreamDirection::Playback,
            capture: None,
        }
    }
}
