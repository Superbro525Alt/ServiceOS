#![no_std]
#![no_main]

use core::{fmt::Write, str};

use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;
use rt::{ControlTag, FixedLogBuffer, LifecycleEvent, LogDomain, LogEvent, LogSeverity, RawMessage, ServiceId};

const REFRESH_TICKS: u64 = 20;

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf201;
    }
    if startup.tag != ControlTag::Startup as u32 || startup.handle_count < 4 || startup.word_count < 3 {
        return 0xf202;
    }

    let surface_handle = startup.handles[0];
    let log_handle = startup.handles[1];
    let status_handle = startup.handles[2];
    let network_handle = startup.handles[3];
    let width = startup.words[1] as u32;
    let height = startup.words[2] as u32;

    let _ = rt::send_log_record(
        log_handle,
        ServiceId::DesktopShell,
        LogSeverity::Info,
        LogDomain::App,
        LogEvent::AppRendered,
        3,
        startup.words[0],
    );

    let mut next_refresh = 0u64;
    loop {
        match poll_lifecycle(bootstrap) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(_) => return 0xf203,
        }

        let now = rt::monotonic_now().unwrap_or(0);
        if now >= next_refresh {
            let _ = render(surface_handle, width, height, status_handle, network_handle, now);
            next_refresh = now.saturating_add(REFRESH_TICKS);
        }

        if rt::yield_current().is_err() {
            return 0xf204;
        }
    }
}

fn render(
    surface_handle: rt::Handle,
    width: u32,
    height: u32,
    status_handle: rt::Handle,
    network_handle: rt::Handle,
    now: u64,
) -> rt::Result<()> {
    let (heartbeat_count, heartbeat_tick) = rt::status_snapshot(status_handle).unwrap_or((0, 0));
    let iface = rt::network_interface_status(network_handle, 0).unwrap_or(None);

    let mut line0 = FixedLogBuffer::<48>::new();
    let _ = write!(&mut line0, "TICK {}", now);
    let mut line1 = FixedLogBuffer::<48>::new();
    let _ = write!(&mut line1, "HEARTBEAT {}", heartbeat_count);
    let mut line2 = FixedLogBuffer::<48>::new();
    let _ = write!(&mut line2, "LAST {}", heartbeat_tick);
    let mut line3 = FixedLogBuffer::<48>::new();
    if let Some(info) = iface {
        let _ = write!(
            &mut line3,
            "ADDR {}.{}.{}.{}",
            (info.address >> 24) & 0xff,
            (info.address >> 16) & 0xff,
            (info.address >> 8) & 0xff,
            info.address & 0xff,
        );
    } else {
        let _ = write!(&mut line3, "ADDR UNAVAILABLE");
    }

    ui::render_window(
        surface_handle,
        width,
        height,
        ui::BG_WINDOW,
        ui::STATUS_OK,
        "MONITOR",
        &[
            str::from_utf8(line0.as_bytes()).unwrap_or("TICK ?"),
            str::from_utf8(line1.as_bytes()).unwrap_or("HEARTBEAT ?"),
            str::from_utf8(line2.as_bytes()).unwrap_or("LAST ?"),
            str::from_utf8(line3.as_bytes()).unwrap_or("ADDR ?"),
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
