#![no_std]
#![no_main]

use core::{fmt::Write, str};

use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;
use rt::{
    AppControlTag, ConfigKey, ControlTag, FixedLogBuffer, LifecycleEvent, RawMessage,
};

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

    if render(surface_handle, width, height, focused, config_handle, network_handle).is_err() {
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
        ],
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
    config_handle: rt::Handle,
    network_handle: rt::Handle,
) -> rt::Result<ControlFlow> {
    let mut message = RawMessage::empty(0);
    match rt::channel_receive_nonblocking(control_handle, &mut message) {
        Ok(()) if message.tag == AppControlTag::FocusChanged as u32 && message.word_count > 0 => {
            *focused = message.words[0] != 0;
            render(surface_handle, *width, *height, *focused, config_handle, network_handle)?;
            Ok(ControlFlow::Continue)
        }
        Ok(()) if message.tag == AppControlTag::Resize as u32 && message.word_count >= 2 => {
            *width = message.words[0] as u32;
            *height = message.words[1] as u32;
            render(surface_handle, *width, *height, *focused, config_handle, network_handle)?;
            Ok(ControlFlow::Continue)
        }
        Ok(()) if message.tag == AppControlTag::Close as u32 => Ok(ControlFlow::Exit),
        Ok(()) => Ok(ControlFlow::Continue),
        Err(rt::Error::QueueEmpty) => Ok(ControlFlow::Continue),
        Err(error) => Err(error),
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
