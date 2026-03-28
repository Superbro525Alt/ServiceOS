#![no_std]
#![no_main]

use core::{char, fmt::Write, str};

use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;
use rt::{
    AppControlTag, AppKeyAction, AppPointerAction, ConfigKey, ControlTag, FixedLogBuffer,
    LifecycleEvent, RawMessage,
};

const NOTE_MAX_BYTES: usize = 24;
const NOTE_FIELD_X0: i32 = 10;
const NOTE_FIELD_Y0: i32 = 98;
const NOTE_FIELD_X1: i32 = 232;
const NOTE_FIELD_Y1: i32 = 122;

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf001;
    }
    if startup.tag != ControlTag::Startup as u32 || startup.handle_count < 4 || startup.word_count < 4 {
        return 0xf002;
    }

    let surface_handle = startup.handles[0];
    let control_handle = startup.handles[1];
    let config_handle = startup.handles[2];
    let network_handle = startup.handles[3];
    let width = startup.words[1] as u32;
    let height = startup.words[2] as u32;
    let mut focused = startup.words[3] != 0;

    let mut width = width;
    let mut height = height;
    let mut editing_note = false;
    let mut note = [0u8; NOTE_MAX_BYTES];
    let mut note_len = 0usize;

    if render(
        surface_handle,
        width,
        height,
        focused,
        config_handle,
        network_handle,
        editing_note,
        &note[..note_len],
    )
    .is_err()
    {
        return 0xf003;
    }
    loop {
        match poll_lifecycle(bootstrap) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(_) => return 0xf004,
        }
        match poll_control(
            control_handle,
            surface_handle,
            &mut width,
            &mut height,
            &mut focused,
            &mut editing_note,
            &mut note,
            &mut note_len,
            config_handle,
            network_handle,
        ) {
            Ok(ControlFlow::Continue) => {}
            Ok(ControlFlow::Exit) => return 0,
            Err(_) => return 0xf006,
        }
        if rt::yield_current().is_err() {
            return 0xf005;
        }
    }
}

fn render(
    surface_handle: rt::Handle,
    width: u32,
    height: u32,
    focused: bool,
    config_handle: rt::Handle,
    network_handle: rt::Handle,
    editing_note: bool,
    note: &[u8],
) -> rt::Result<()> {
    let min_level = rt::config_read(config_handle, ConfigKey::LogMinimumSeverity)
        .map(|(_, value)| value)
        .unwrap_or(0);
    let heartbeat = rt::config_read(config_handle, ConfigKey::StatusHeartbeatTicks)
        .map(|(_, value)| value)
        .unwrap_or(0);
    let interface = rt::network_interface_status(network_handle, 0).unwrap_or(None);

    let mut line0 = FixedLogBuffer::<48>::new();
    let _ = write!(&mut line0, "LOG MINIMUM {}", min_level);
    let mut line1 = FixedLogBuffer::<48>::new();
    let _ = write!(&mut line1, "HEARTBEAT {}", heartbeat);
    let mut line2 = FixedLogBuffer::<48>::new();
    if let Some(info) = interface {
        let _ = write!(
            &mut line2,
            "IP {}.{}.{}.{}",
            (info.address >> 24) & 0xff,
            (info.address >> 16) & 0xff,
            (info.address >> 8) & 0xff,
            info.address & 0xff,
        );
    } else {
        let _ = write!(&mut line2, "IP UNAVAILABLE");
    }
    let mut line3 = FixedLogBuffer::<48>::new();
    if let Some(info) = interface {
        let _ = write!(
            &mut line3,
            "GATEWAY {}.{}.{}.{}",
            (info.gateway >> 24) & 0xff,
            (info.gateway >> 16) & 0xff,
            (info.gateway >> 8) & 0xff,
            info.gateway & 0xff,
        );
    } else {
        let _ = write!(&mut line3, "GATEWAY UNAVAILABLE");
    }
    let mut line4 = FixedLogBuffer::<48>::new();
    let note_prefix = if editing_note { "NOTE * " } else { "NOTE   " };
    let note_text = str::from_utf8(note).unwrap_or("");
    let _ = write!(&mut line4, "{}{}", note_prefix, note_text);
    let mut line5 = FixedLogBuffer::<48>::new();
    let _ = write!(&mut line5, "CLICK NOTE FIELD, TYPE TEXT");

    ui::render_window_state(
        surface_handle,
        width,
        height,
        ui::BG_WINDOW,
        ui::ACCENT,
        "SETTINGS",
        &[
            str::from_utf8(line0.as_bytes()).unwrap_or("LOG MINIMUM ?"),
            str::from_utf8(line1.as_bytes()).unwrap_or("HEARTBEAT ?"),
            str::from_utf8(line2.as_bytes()).unwrap_or("IP ?"),
            str::from_utf8(line3.as_bytes()).unwrap_or("GATEWAY ?"),
            str::from_utf8(line4.as_bytes()).unwrap_or("NOTE"),
            str::from_utf8(line5.as_bytes()).unwrap_or("TYPE"),
        ],
        focused,
    )?;
    rt::surface_set_rect(
        surface_handle,
        7,
        NOTE_FIELD_X0,
        NOTE_FIELD_Y0,
        (NOTE_FIELD_X1 - NOTE_FIELD_X0) as u32,
        (NOTE_FIELD_Y1 - NOTE_FIELD_Y0) as u32,
        if editing_note { ui::ACCENT } else { ui::ACCENT_DIM },
        true,
    )?;
    rt::surface_set_label(
        surface_handle,
        11,
        NOTE_FIELD_X0 + 8,
        NOTE_FIELD_Y0 + 7,
        ui::BG_PANEL,
        note_text,
    )?;
    let _ = height;
    Ok(())
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
    editing_note: &mut bool,
    note: &mut [u8; NOTE_MAX_BYTES],
    note_len: &mut usize,
    config_handle: rt::Handle,
    network_handle: rt::Handle,
) -> rt::Result<ControlFlow> {
    let mut changed = false;
    loop {
        let mut message = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(control_handle, &mut message) {
            Ok(()) if message.tag == AppControlTag::FocusChanged as u32 && message.word_count > 0 => {
                *focused = message.words[0] != 0;
                changed = true;
            }
            Ok(()) if message.tag == AppControlTag::Resize as u32 && message.word_count >= 2 => {
                *width = message.words[0] as u32;
                *height = message.words[1] as u32;
                changed = true;
            }
            Ok(()) if message.tag == AppControlTag::Pointer as u32 && message.word_count >= 4 => {
                let action = app_pointer_action_from_word(message.words[0]);
                let x = message.words[1] as i64 as i32;
                let y = message.words[2] as i64 as i32;
                if matches!(action, Some(AppPointerAction::Down))
                    && x >= NOTE_FIELD_X0
                    && x < NOTE_FIELD_X1
                    && y >= NOTE_FIELD_Y0
                    && y < NOTE_FIELD_Y1
                {
                    *editing_note = true;
                    changed = true;
                } else if matches!(action, Some(AppPointerAction::Down)) {
                    *editing_note = false;
                    changed = true;
                }
            }
            Ok(()) if message.tag == AppControlTag::Key as u32 && message.word_count >= 2 => {
                if *editing_note
                    && matches!(app_key_action_from_word(message.words[0]), Some(AppKeyAction::Down))
                    && message.words[1] as u32 == 14
                    && *note_len > 0
                {
                    *note_len -= 1;
                    changed = true;
                }
            }
            Ok(()) if message.tag == AppControlTag::Text as u32 && message.word_count > 0 => {
                if *editing_note && let Some(ch) = char::from_u32(message.words[0] as u32) {
                    if ch == '\n' {
                        *editing_note = false;
                        changed = true;
                    } else if ch.is_ascii_graphic() || ch == ' ' {
                        let mut scratch = [0u8; 4];
                        let bytes = ch.encode_utf8(&mut scratch).as_bytes();
                        if *note_len + bytes.len() <= NOTE_MAX_BYTES {
                            note[*note_len..*note_len + bytes.len()].copy_from_slice(bytes);
                            *note_len += bytes.len();
                            changed = true;
                        }
                    }
                }
            }
            Ok(()) if message.tag == AppControlTag::Close as u32 => return Ok(ControlFlow::Exit),
            Ok(()) => {}
            Err(rt::Error::QueueEmpty) => break,
            Err(error) => return Err(error),
        }
    }

    if changed {
        render(
            surface_handle,
            *width,
            *height,
            *focused,
            config_handle,
            network_handle,
            *editing_note,
            &note[..*note_len],
        )?;
    }

    Ok(ControlFlow::Continue)
}

fn app_pointer_action_from_word(value: u64) -> Option<AppPointerAction> {
    match value as u32 {
        x if x == AppPointerAction::Down as u32 => Some(AppPointerAction::Down),
        x if x == AppPointerAction::Move as u32 => Some(AppPointerAction::Move),
        x if x == AppPointerAction::Up as u32 => Some(AppPointerAction::Up),
        _ => None,
    }
}

fn app_key_action_from_word(value: u64) -> Option<AppKeyAction> {
    match value as u32 {
        x if x == AppKeyAction::Down as u32 => Some(AppKeyAction::Down),
        x if x == AppKeyAction::Up as u32 => Some(AppKeyAction::Up),
        _ => None,
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
