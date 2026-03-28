#![no_std]
#![no_main]

use core::{fmt::Write, str};

use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;
use rt::{
    AppControlTag, ControlTag, FixedLogBuffer, LifecycleEvent, RawMessage,
};

const REFRESH_TICKS: u64 = 100;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MonitorSnapshot {
    heartbeat_count: u64,
    heartbeat_tick: u64,
    ipv4_address: u32,
    link_up: bool,
}

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf201;
    }
    if startup.tag != ControlTag::Startup as u32 || startup.handle_count < 4 || startup.word_count < 4 {
        return 0xf202;
    }

    let surface_handle = startup.handles[0];
    let control_handle = startup.handles[1];
    let status_handle = startup.handles[2];
    let network_handle = startup.handles[3];
    let width = startup.words[1] as u32;
    let height = startup.words[2] as u32;
    let mut focused = startup.words[3] != 0;
    let mut width = width;
    let mut height = height;

    let mut next_refresh = 0u64;
    let mut last_snapshot: Option<MonitorSnapshot> = None;
    loop {
        match poll_lifecycle(bootstrap) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(_) => return 0xf203,
        }

        match poll_control(control_handle, &mut width, &mut height, &mut focused) {
            Ok(ControlFlow::Continue) => {}
            Ok(ControlFlow::Exit) => return 0,
            Err(_) => return 0xf205,
        }

        let now = rt::monotonic_now().unwrap_or(0);
        if now >= next_refresh {
            let snapshot = sample_snapshot(status_handle, network_handle);
            if last_snapshot != Some(snapshot) {
                let _ = render(surface_handle, width, height, focused, snapshot);
                last_snapshot = Some(snapshot);
            }
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
    focused: bool,
    snapshot: MonitorSnapshot,
) -> rt::Result<()> {
    let mut line0 = FixedLogBuffer::<48>::new();
    let _ = write!(&mut line0, "HEARTBEAT {}", snapshot.heartbeat_count);
    let mut line1 = FixedLogBuffer::<48>::new();
    let _ = write!(&mut line1, "LAST {}", snapshot.heartbeat_tick);
    let mut line2 = FixedLogBuffer::<48>::new();
    if snapshot.ipv4_address != 0 {
        let _ = write!(
            &mut line2,
            "ADDR {}.{}.{}.{}",
            (snapshot.ipv4_address >> 24) & 0xff,
            (snapshot.ipv4_address >> 16) & 0xff,
            (snapshot.ipv4_address >> 8) & 0xff,
            snapshot.ipv4_address & 0xff,
        );
    } else {
        let _ = write!(&mut line2, "ADDR UNAVAILABLE");
    }
    let mut line3 = FixedLogBuffer::<48>::new();
    if snapshot.link_up {
        let _ = write!(&mut line3, "LINK UP");
    } else {
        let _ = write!(&mut line3, "LINK DOWN");
    }

    ui::render_window_state(
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
        focused,
    )
}

enum ControlFlow {
    Continue,
    Exit,
}

fn poll_control(
    control_handle: rt::Handle,
    width: &mut u32,
    height: &mut u32,
    focused: &mut bool,
) -> rt::Result<ControlFlow> {
    loop {
        let mut message = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(control_handle, &mut message) {
            Ok(()) if message.tag == AppControlTag::FocusChanged as u32 && message.word_count > 0 => {
                *focused = message.words[0] != 0;
            }
            Ok(()) if message.tag == AppControlTag::Resize as u32 && message.word_count >= 2 => {
                *width = message.words[0] as u32;
                *height = message.words[1] as u32;
            }
            Ok(()) if message.tag == AppControlTag::Close as u32 => return Ok(ControlFlow::Exit),
            Ok(()) => {}
            Err(rt::Error::QueueEmpty) => break,
            Err(error) => return Err(error),
        }
    }

    Ok(ControlFlow::Continue)
}

fn sample_snapshot(status_handle: rt::Handle, network_handle: rt::Handle) -> MonitorSnapshot {
    let (heartbeat_count, heartbeat_tick) = rt::status_snapshot(status_handle).unwrap_or((0, 0));
    let interface = rt::network_interface_status(network_handle, 0)
        .ok()
        .flatten();
    MonitorSnapshot {
        heartbeat_count,
        heartbeat_tick,
        ipv4_address: interface.map(|info| info.address).unwrap_or(0),
        link_up: interface
            .map(|info| info.link_state == rt::PacketInterfaceLinkState::Up)
            .unwrap_or(false),
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
