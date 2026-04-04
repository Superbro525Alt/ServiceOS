#![no_std]
#![no_main]

use core::{fmt::Write, str};

use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;
use rt::{AppControlTag, ControlTag, FixedLogBuffer, RawMessage};

const REFRESH_TICKS: u64 = 100;
const BUFFER_WIDTH: u32 = 640;
const BUFFER_HEIGHT: u32 = 480;
const BUFFER_BYTES: usize = BUFFER_WIDTH as usize * BUFFER_HEIGHT as usize * 4;
const SURFACE_BUFFER_SLOTS: usize = 2;

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

    let mut buffers = match ui::SurfaceBuffers::<SURFACE_BUFFER_SLOTS>::new(
        surface_handle,
        BUFFER_WIDTH,
        BUFFER_HEIGHT,
        BUFFER_WIDTH,
        BUFFER_BYTES,
    ) {
        Ok(buffers) => buffers,
        Err(_) => return 0xf206,
    };

    let mut next_refresh = 0u64;
    let mut last_snapshot: Option<MonitorSnapshot> = None;
    loop {
        match ui::poll_app_lifecycle(bootstrap) {
            Ok(true) => break,
            Ok(false) => {}
            Err(_) => return 0xf203,
        }

        match poll_control(control_handle, &mut width, &mut height, &mut focused) {
            Ok(ControlFlow::Idle) => {}
            Ok(ControlFlow::Worked) => {
                if let Some(snapshot) = last_snapshot {
                    let (slot, buffer) = buffers.advance();
                    let _ = render(
                        surface_handle,
                        slot,
                        buffer,
                        width,
                        height,
                        focused,
                        snapshot,
                    );
                }
                continue;
            }
            Ok(ControlFlow::Exit) => break,
            Err(_) => return 0xf205,
        }

        let now = rt::monotonic_now().unwrap_or(0);
        if now >= next_refresh {
            let snapshot = sample_snapshot(status_handle, network_handle);
            if last_snapshot != Some(snapshot) {
                let (slot, buffer) = buffers.advance();
                let _ = render(
                    surface_handle,
                    slot,
                    buffer,
                    width,
                    height,
                    focused,
                    snapshot,
                );
                last_snapshot = Some(snapshot);
            }
            next_refresh = now.saturating_add(REFRESH_TICKS);
        }

        if rt::yield_current().is_err() {
            return 0xf204;
        }
    }

    0
}

fn render(
    surface_handle: rt::Handle,
    buffer_slot: u32,
    buffer: &mut rt::MappedMemory,
    width: u32,
    height: u32,
    focused: bool,
    snapshot: MonitorSnapshot,
) -> rt::Result<()> {
    render_buffer(buffer.as_slice_mut(), width, height, snapshot);

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
    )?;
    rt::surface_present_buffer_slot(
        surface_handle,
        buffer_slot,
        0,
        0,
        width.min(BUFFER_WIDTH),
        height.min(BUFFER_HEIGHT),
    )
}

enum ControlFlow {
    Idle,
    Worked,
    Exit,
}

fn poll_control(
    control_handle: rt::Handle,
    width: &mut u32,
    height: &mut u32,
    focused: &mut bool,
) -> rt::Result<ControlFlow> {
    let mut did_work = false;
    loop {
        let mut message = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(control_handle, &mut message) {
            Ok(()) if message.tag == AppControlTag::FocusChanged as u32 && message.word_count > 0 => {
                did_work = true;
                *focused = message.words[0] != 0;
            }
            Ok(()) if message.tag == AppControlTag::Resize as u32 && message.word_count >= 2 => {
                did_work = true;
                *width = message.words[0] as u32;
                *height = message.words[1] as u32;
            }
            Ok(()) if message.tag == AppControlTag::Close as u32 => return Ok(ControlFlow::Exit),
            Ok(()) => {
                did_work = true;
            }
            Err(rt::Error::QueueEmpty) => break,
            Err(error) => return Err(error),
        }
    }

    if did_work {
        Ok(ControlFlow::Worked)
    } else {
        Ok(ControlFlow::Idle)
    }
}

fn render_buffer(bytes: &mut [u8], width: u32, height: u32, snapshot: MonitorSnapshot) {
    let width = width.min(BUFFER_WIDTH) as usize;
    let height = height.min(BUFFER_HEIGHT) as usize;
    if width == 0 || height == 0 {
        return;
    }
    for y in 0..height {
        let blue = 0x18 + ((y * 20) / height.max(1)) as u32;
        let rgb = (0x16 << 16) | (0x21 << 8) | blue;
        for x in 0..width {
            set_pixel(bytes, x, y, rgb);
        }
    }

    fill_rect(bytes, 12, 52, width.saturating_sub(24), 14, if snapshot.link_up { ui::STATUS_OK } else { ui::STATUS_WARN });
    fill_rect(bytes, 18, 84, width.saturating_sub(36), 18, 0x24334a);

    let meter_width = width.saturating_sub(36);
    let heartbeat_fill = ((snapshot.heartbeat_count as usize) % meter_width.max(1)) + 1;
    fill_rect(bytes, 18, 84, heartbeat_fill.min(meter_width), 18, ui::ACCENT);

    let octets = [
        ((snapshot.ipv4_address >> 24) & 0xff) as usize,
        ((snapshot.ipv4_address >> 16) & 0xff) as usize,
        ((snapshot.ipv4_address >> 8) & 0xff) as usize,
        (snapshot.ipv4_address & 0xff) as usize,
    ];
    for (index, octet) in octets.iter().copied().enumerate() {
        let bar_height = 12 + (octet * 56 / 255);
        let x = 24 + (index * 28);
        let y = height.saturating_sub(32 + bar_height);
        fill_rect(bytes, x, y, 18, bar_height, ui::STATUS_OK);
    }
}

fn fill_rect(bytes: &mut [u8], x: usize, y: usize, width: usize, height: usize, rgb: u32) {
    let end_x = (x + width).min(BUFFER_WIDTH as usize);
    let end_y = (y + height).min(BUFFER_HEIGHT as usize);
    for py in y..end_y {
        for px in x..end_x {
            set_pixel(bytes, px, py, rgb);
        }
    }
}

fn set_pixel(bytes: &mut [u8], x: usize, y: usize, rgb: u32) {
    let index = ((y * BUFFER_WIDTH as usize) + x) * 4;
    if index + 4 > bytes.len() {
        return;
    }
    bytes[index..index + 4].copy_from_slice(&(rgb & 0x00ff_ffff).to_le_bytes());
}

fn sample_snapshot(status_handle: rt::Handle, network_handle: rt::Handle) -> MonitorSnapshot {
    let (heartbeat_count, heartbeat_tick, _) =
        rt::status_snapshot(status_handle).unwrap_or((0, 0, 0));
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
