#![no_std]
#![no_main]

use core::{fmt::Write, str};

use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;
use rt::{ConfigKey, ControlTag, FixedLogBuffer, LifecycleEvent, LogDomain, LogEvent, LogSeverity, RawMessage, ServiceId};

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf001;
    }
    if startup.tag != ControlTag::Startup as u32 || startup.handle_count < 4 || startup.word_count < 3 {
        return 0xf002;
    }

    let surface_handle = startup.handles[0];
    let log_handle = startup.handles[1];
    let config_handle = startup.handles[2];
    let network_handle = startup.handles[3];
    let width = startup.words[1] as u32;
    let height = startup.words[2] as u32;

    if render(surface_handle, width, height, config_handle, network_handle).is_err() {
        return 0xf003;
    }
    let _ = rt::send_log_record(
        log_handle,
        ServiceId::DesktopShell,
        LogSeverity::Info,
        LogDomain::App,
        LogEvent::AppRendered,
        1,
        startup.words[0],
    );

    loop {
        match poll_lifecycle(bootstrap) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(_) => return 0xf004,
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

    ui::render_window(
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
    )
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
