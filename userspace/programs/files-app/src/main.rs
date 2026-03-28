#![no_std]
#![no_main]

use core::str;

use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;
use rt::{AppControlTag, ControlTag, LifecycleEvent, RawMessage};

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf101;
    }
    if startup.tag != ControlTag::Startup as u32 || startup.handle_count < 3 || startup.word_count < 4 {
        return 0xf102;
    }

    let surface_handle = startup.handles[0];
    let control_handle = startup.handles[1];
    let storage_handle = startup.handles[2];
    let width = startup.words[1] as u32;
    let height = startup.words[2] as u32;
    let mut focused = startup.words[3] != 0;
    let mut width = width;
    let mut height = height;

    if render(surface_handle, width, height, focused, storage_handle).is_err() {
        return 0xf103;
    }
    loop {
        match poll_lifecycle(bootstrap) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(_) => return 0xf104,
        }
        match poll_control(
            control_handle,
            surface_handle,
            &mut width,
            &mut height,
            &mut focused,
            storage_handle,
        ) {
            Ok(ControlFlow::Continue) => {}
            Ok(ControlFlow::Exit) => return 0,
            Err(_) => return 0xf106,
        }
        if rt::yield_current().is_err() {
            return 0xf105;
        }
    }
}

fn render(
    surface_handle: rt::Handle,
    width: u32,
    height: u32,
    focused: bool,
    storage_handle: rt::Handle,
) -> rt::Result<()> {
    let mut path_bytes = [[0u8; 64]; 4];
    let mut path_lens = [0usize; 4];
    let mut line_states = [FileLine::Pending; 4];
    for index in 0..4 {
        match rt::storage_list(storage_handle, "", index, &mut path_bytes[index]) {
            Ok(Some((_status, len))) => {
                path_lens[index] = len;
                line_states[index] = FileLine::Path;
            }
            Ok(None) => {
                line_states[index] = FileLine::End;
                break;
            }
            Err(_) => {
                line_states[index] = FileLine::Failed;
                break;
            }
        }
    }

    let line0 = file_line_text(line_states[0], &path_bytes[0], path_lens[0]);
    let line1 = file_line_text(line_states[1], &path_bytes[1], path_lens[1]);
    let line2 = file_line_text(line_states[2], &path_bytes[2], path_lens[2]);
    let line3 = file_line_text(line_states[3], &path_bytes[3], path_lens[3]);

    ui::render_window_state(
        surface_handle,
        width,
        height,
        ui::BG_WINDOW_ALT,
        ui::ACCENT_DIM,
        "FILES",
        &[line0, line1, line2, line3],
        focused,
    )
}

enum ControlFlow {
    Continue,
    Exit,
}

fn poll_control(
    control_handle: rt::Handle,
    surface_handle: rt::Handle,
    width: &mut u32,
    height: &mut u32,
    focused: &mut bool,
    storage_handle: rt::Handle,
) -> rt::Result<ControlFlow> {
    let mut message = RawMessage::empty(0);
    match rt::channel_receive_nonblocking(control_handle, &mut message) {
        Ok(()) if message.tag == AppControlTag::FocusChanged as u32 && message.word_count > 0 => {
            *focused = message.words[0] != 0;
            render(surface_handle, *width, *height, *focused, storage_handle)?;
            Ok(ControlFlow::Continue)
        }
        Ok(()) if message.tag == AppControlTag::Resize as u32 && message.word_count >= 2 => {
            *width = message.words[0] as u32;
            *height = message.words[1] as u32;
            render(surface_handle, *width, *height, *focused, storage_handle)?;
            Ok(ControlFlow::Continue)
        }
        Ok(()) if message.tag == AppControlTag::Close as u32 => Ok(ControlFlow::Exit),
        Ok(()) => Ok(ControlFlow::Continue),
        Err(rt::Error::QueueEmpty) => Ok(ControlFlow::Continue),
        Err(error) => Err(error),
    }
}

#[derive(Clone, Copy)]
enum FileLine {
    Pending,
    Path,
    End,
    Failed,
}

fn file_line_text<'a>(state: FileLine, bytes: &'a [u8], len: usize) -> &'a str {
    match state {
        FileLine::Pending => "BOOT STORE",
        FileLine::Path => str::from_utf8(&bytes[..len]).unwrap_or("INVALID"),
        FileLine::End => "END",
        FileLine::Failed => "LIST FAILED",
    }
}

fn poll_lifecycle(bootstrap: rt::Handle) -> rt::Result<bool> {
    let mut message = RawMessage::empty(0);
    match rt::channel_receive_nonblocking(bootstrap, &mut message) {
        Ok(()) if message.tag == ControlTag::Lifecycle as u32 && message.word_count > 0 => Ok(
            matches!(
                lifecycle_event_from_word(message.words[0]),
                LifecycleEvent::Restarting | LifecycleEvent::Stopped
            ),
        ),
        Ok(()) => Ok(false),
        Err(rt::Error::QueueEmpty) => Ok(false),
        Err(error) => Err(error),
    }
}

fn lifecycle_event_from_word(value: u64) -> LifecycleEvent {
    match value as u32 {
        x if x == LifecycleEvent::Starting as u32 => LifecycleEvent::Starting,
        x if x == LifecycleEvent::Ready as u32 => LifecycleEvent::Ready,
        x if x == LifecycleEvent::Failed as u32 => LifecycleEvent::Failed,
        x if x == LifecycleEvent::Stopped as u32 => LifecycleEvent::Stopped,
        _ => LifecycleEvent::Restarting,
    }
}
