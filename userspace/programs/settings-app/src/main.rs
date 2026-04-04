#![no_std]
#![no_main]

mod control;
mod render;
mod security;
mod state;

use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;
use rt::ControlTag;

use crate::control::{cleanup_audio, poll_control, ControlFlow};
use crate::render::render;
use crate::state::{AppState, BUFFER_BYTES, BUFFER_HEIGHT, BUFFER_WIDTH, NOTE_MAX_BYTES, SURFACE_BUFFER_SLOTS};

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = rt::RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf001;
    }
    if startup.tag != ControlTag::Startup as u32 || startup.handle_count < 6 || startup.word_count < 4 {
        return 0xf002;
    }

    let surface_handle = startup.handles[0];
    let control_handle = startup.handles[1];
    let config_handle = startup.handles[2];
    let network_handle = startup.handles[3];
    let audio_handle = startup.handles[4];
    let security_handle = startup.handles[5];
    let runtime_handle = if startup.handle_count >= 7 {
        startup.handles[6]
    } else {
        rt::INVALID_HANDLE
    };
    let mut state = AppState {
        width: startup.words[1] as u32,
        height: startup.words[2] as u32,
        focused: startup.words[3] != 0,
        page: state::SettingsPage::System,
        editing_note: false,
        selected_policy_index: 0,
        note: [0; NOTE_MAX_BYTES],
        note_len: 0,
    };
    let audio_stream_handle =
        rt::audio_stream_open(audio_handle, rt::AudioStreamDirection::Playback, 0)
            .unwrap_or(rt::INVALID_HANDLE);

    let mut buffers = match ui::SurfaceBuffers::<SURFACE_BUFFER_SLOTS>::new(
        surface_handle,
        BUFFER_WIDTH,
        BUFFER_HEIGHT,
        BUFFER_WIDTH,
        BUFFER_BYTES,
    ) {
        Ok(buffers) => buffers,
        Err(_) => return 0xf007,
    };

    let (slot, buffer) = buffers.current();
    if render(
        surface_handle,
        slot,
        buffer,
        config_handle,
        network_handle,
        audio_handle,
        runtime_handle,
        security_handle,
        &state,
    )
    .is_err()
    {
        return 0xf003;
    }

    loop {
        match ui::poll_app_lifecycle(bootstrap) {
            Ok(true) => {
                cleanup_audio(audio_stream_handle, audio_handle);
                break;
            }
            Ok(false) => {}
            Err(_) => return 0xf004,
        }
        match poll_control(
            control_handle,
            surface_handle,
            &mut buffers,
            config_handle,
            network_handle,
            audio_handle,
            runtime_handle,
            security_handle,
            audio_stream_handle,
            &mut state,
        ) {
            Ok(ControlFlow::Continue) => {}
            Ok(ControlFlow::Exit) => {
                cleanup_audio(audio_stream_handle, audio_handle);
                break;
            }
            Err(_) => return 0xf006,
        }
        if rt::yield_current().is_err() {
            cleanup_audio(audio_stream_handle, audio_handle);
            return 0xf005;
        }
    }

    0
}
