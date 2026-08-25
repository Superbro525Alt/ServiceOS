use alloc::sync::Arc;
use spin::Mutex;
use x86_64::instructions::port::Port;

use crate::sound;
use serviceos_abi::{
    AudioEndpointBackend, AudioEndpointDirection, AudioEndpointInfo, AudioEndpointState,
    AudioToneRequest, audio_capability,
};
use serviceos_kernel_core::{
    audio::{AudioBackend, AudioEndpointError},
    time,
};

const PIT_COMMAND_PORT: u16 = 0x43;
const PIT_CHANNEL2_PORT: u16 = 0x42;
const SPEAKER_CONTROL_PORT: u16 = 0x61;
const PIT_INPUT_HZ: u32 = 1_193_182;
const MIN_FREQUENCY_HZ: u32 = 37;
const MAX_FREQUENCY_HZ: u32 = 20_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioBringupSummary {
    pub backend: AudioEndpointBackend,
    pub default_frequency_hz: u32,
}

#[derive(Clone, Copy)]
struct EndpointState {
    current_frequency_hz: u32,
    active_until_tick: u64,
    play_count: u64,
}

pub struct PcSpeakerBackend {
    state: Mutex<EndpointState>,
}

static AUDIO_BRINGUP: Mutex<Option<AudioBringupSummary>> = Mutex::new(None);

impl PcSpeakerBackend {
    fn new() -> Self {
        Self {
            state: Mutex::new(EndpointState {
                current_frequency_hz: 0,
                active_until_tick: 0,
                play_count: 0,
            }),
        }
    }

    fn refresh_locked(state: &mut EndpointState) {
        let now = time::manager().map(|manager| manager.now().0).unwrap_or(0);
        if state.active_until_tick != 0 && now >= state.active_until_tick {
            disable_speaker();
            state.current_frequency_hz = 0;
            state.active_until_tick = 0;
        }
    }
}

impl AudioBackend for PcSpeakerBackend {
    fn info(&self) -> AudioEndpointInfo {
        let mut state = self.state.lock();
        Self::refresh_locked(&mut state);
        AudioEndpointInfo {
            backend: AudioEndpointBackend::PcSpeaker as u32,
            direction: AudioEndpointDirection::Output as u32,
            state: if state.current_frequency_hz == 0 {
                AudioEndpointState::Idle as u32
            } else {
                AudioEndpointState::Active as u32
            },
            capabilities: audio_capability::PLAYBACK | audio_capability::TONE,
            nominal_rate_hz: PIT_INPUT_HZ,
            channels: 1,
            min_frequency_hz: MIN_FREQUENCY_HZ,
            max_frequency_hz: MAX_FREQUENCY_HZ,
            current_frequency_hz: state.current_frequency_hz,
            reserved: 0,
            play_count: state.play_count,
        }
    }

    fn play_tone(&self, request: AudioToneRequest) -> Result<(), AudioEndpointError> {
        let frequency_hz = request
            .frequency_hz
            .clamp(MIN_FREQUENCY_HZ, MAX_FREQUENCY_HZ);
        if request.duration_ticks == 0 {
            return self.stop();
        }

        program_speaker(frequency_hz);
        let mut state = self.state.lock();
        state.current_frequency_hz = frequency_hz;
        state.play_count = state.play_count.saturating_add(1);
        let now = time::manager().map(|manager| manager.now().0).unwrap_or(0);
        state.active_until_tick = now.saturating_add(request.duration_ticks as u64);
        Ok(())
    }

    fn stop(&self) -> Result<(), AudioEndpointError> {
        disable_speaker();
        let mut state = self.state.lock();
        state.current_frequency_hz = 0;
        state.active_until_tick = 0;
        Ok(())
    }
}

pub fn initialize() -> Arc<dyn AudioBackend> {
    // Prefer a real PCM sink (QEMU virtio-sound) when the device is
    // present; fall back to the tone-only PC speaker otherwise.
    if let Some(backend) = sound::initialize() {
        *AUDIO_BRINGUP.lock() = Some(AudioBringupSummary {
            backend: AudioEndpointBackend::VirtioSound,
            default_frequency_hz: 0,
        });
        return backend;
    }
    let backend = Arc::new(PcSpeakerBackend::new());
    *AUDIO_BRINGUP.lock() = Some(AudioBringupSummary {
        backend: AudioEndpointBackend::PcSpeaker,
        default_frequency_hz: 880,
    });
    backend
}

pub fn bringup_summary() -> Option<AudioBringupSummary> {
    *AUDIO_BRINGUP.lock()
}

fn program_speaker(frequency_hz: u32) {
    let divisor = (PIT_INPUT_HZ / frequency_hz.max(1)).clamp(1, u16::MAX as u32) as u16;
    let low = (divisor & 0xff) as u8;
    let high = (divisor >> 8) as u8;
    let mut command = Port::<u8>::new(PIT_COMMAND_PORT);
    let mut channel2 = Port::<u8>::new(PIT_CHANNEL2_PORT);
    let mut control = Port::<u8>::new(SPEAKER_CONTROL_PORT);

    // SAFETY: These are the platform-specific PIT and speaker control ports for the current
    // x86_64 QEMU target.
    unsafe {
        command.write(0xB6);
        channel2.write(low);
        channel2.write(high);
        let value = control.read();
        control.write(value | 0x03);
    }
}

fn disable_speaker() {
    let mut control = Port::<u8>::new(SPEAKER_CONTROL_PORT);
    // SAFETY: This is the platform-specific speaker control port for the current target.
    unsafe {
        let value = control.read();
        control.write(value & !0x03);
    }
}
