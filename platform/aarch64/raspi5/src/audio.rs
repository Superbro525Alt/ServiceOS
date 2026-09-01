//! Raspberry Pi 5 audio bring-up.
//!
//! No virtio-sound or Pi 5 audio transport is implemented in-tree, so the
//! endpoint exists purely so the graphical service graph's optional
//! audio-service can come up, report capabilities, and run its software mix
//! selftest. Writes are accepted and counted honestly (discarded). This
//! mirrors the QEMU virt platform's null sink one-for-one. UNTESTED WITHOUT
//! HARDWARE — nothing here can be exercised until a real Pi 5 boots the
//! image (raspi5 is ManualDeploy in QEMU); a real I2S/HDMI audio backend
//! replaces this behind the same object.

use alloc::sync::Arc;
use spin::Mutex;

use serviceos_abi::{
    AudioEndpointBackend, AudioEndpointDirection, AudioEndpointInfo, AudioEndpointState,
    AudioToneRequest, audio_capability,
};
use serviceos_kernel_core::audio::{AudioBackend, AudioEndpointError};

/// Null PCM/tone sink for the Pi 5.
const NULL_SINK_RATE_HZ: u32 = 48_000;

struct EndpointState {
    active: bool,
    play_count: u64,
    bytes_accepted: u64,
}

pub struct NullSinkBackend {
    state: Mutex<EndpointState>,
}

impl NullSinkBackend {
    fn new() -> Self {
        Self {
            state: Mutex::new(EndpointState {
                active: false,
                play_count: 0,
                bytes_accepted: 0,
            }),
        }
    }
}

impl AudioBackend for NullSinkBackend {
    fn info(&self) -> AudioEndpointInfo {
        let state = self.state.lock();
        AudioEndpointInfo {
            backend: AudioEndpointBackend::Unknown as u32,
            direction: AudioEndpointDirection::Output as u32,
            state: if state.active {
                AudioEndpointState::Active as u32
            } else {
                AudioEndpointState::Idle as u32
            },
            capabilities: audio_capability::PLAYBACK | audio_capability::TONE,
            nominal_rate_hz: NULL_SINK_RATE_HZ,
            channels: 2,
            min_frequency_hz: 0,
            max_frequency_hz: 0,
            current_frequency_hz: 0,
            reserved: 0,
            play_count: state.play_count,
        }
    }

    fn play_tone(&self, request: AudioToneRequest) -> Result<(), AudioEndpointError> {
        let mut state = self.state.lock();
        state.active = request.duration_ticks != 0;
        if state.active {
            state.play_count = state.play_count.saturating_add(1);
        }
        Ok(())
    }

    fn stop(&self) -> Result<(), AudioEndpointError> {
        let mut state = self.state.lock();
        state.active = false;
        Ok(())
    }

    fn pcm_write_s16le_stereo(&self, bytes: &[u8]) -> Result<usize, AudioEndpointError> {
        if bytes.is_empty() || bytes.len() % 4 != 0 {
            return Err(AudioEndpointError::Unsupported);
        }
        let mut state = self.state.lock();
        state.bytes_accepted = state.bytes_accepted.saturating_add(bytes.len() as u64);
        Ok(bytes.len())
    }
}

pub fn initialize() -> Arc<dyn AudioBackend> {
    Arc::new(NullSinkBackend::new())
}
