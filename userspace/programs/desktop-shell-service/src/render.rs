use core::{fmt::Write, str};

use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;
use rt::FixedLogBuffer;

use crate::{
    windows::{app_title, launcher_line, running_app_count},
    DesktopState, DesktopStatusSnapshot, CURSOR_SIZE, CURSOR_Z_ORDER, LAUNCHER_HEIGHT,
    LAUNCHER_WIDTH, STATUS_PANEL_HEIGHT, STATUS_PANEL_WIDTH, TOPBAR_HEIGHT,
};

pub(crate) fn render_desktop(state: &mut DesktopState) -> rt::Result<()> {
    let status_snapshot = sample_desktop_status(state);
    rt::surface_set_fill(state.chrome.desktop_handle, ui::BG_DESKTOP)?;
    rt::surface_clear_scene(state.chrome.desktop_handle)?;

    let mut running_buf = FixedLogBuffer::<32>::new();
    let _ = write!(&mut running_buf, "RUNNING {}", status_snapshot.running_apps);
    let running_text = str::from_utf8(running_buf.as_bytes()).unwrap_or("RUNNING ?");

    let mut focus_buf = FixedLogBuffer::<40>::new();
    let _ = write!(
        &mut focus_buf,
        "FOCUS {}",
        status_snapshot.focused_app.map(app_title).unwrap_or("NONE")
    );
    let focus_text = str::from_utf8(focus_buf.as_bytes()).unwrap_or("FOCUS ?");

    let mut network_buf = FixedLogBuffer::<48>::new();
    write_network_status(&mut network_buf, status_snapshot.ipv4_address);
    let network_text = str::from_utf8(network_buf.as_bytes()).unwrap_or("NET OFFLINE");
    let notification_text = if state.notification_len != 0 {
        str::from_utf8(&state.notification[..state.notification_len]).unwrap_or("NOTICE")
    } else {
        "NO NOTIFICATIONS"
    };

    ui::render_panel(
        state.chrome.topbar_handle,
        state.chrome.output_width,
        TOPBAR_HEIGHT,
        "SERVICEOS DESKTOP",
        &[running_text, focus_text, notification_text],
    )?;

    let launcher_lines = [
        launcher_line(state.apps[0]),
        launcher_line(state.apps[1]),
        launcher_line(state.apps[2]),
        launcher_line(state.apps[3]),
        launcher_line(state.apps[4]),
    ];
    ui::render_panel_uniform(
        state.chrome.launcher_handle,
        LAUNCHER_WIDTH,
        LAUNCHER_HEIGHT,
        "APPS",
        &launcher_lines,
        ui::TEXT_PRIMARY,
    )?;

    let mut hb_buf = FixedLogBuffer::<32>::new();
    let _ = write!(&mut hb_buf, "HEARTBEAT {}", status_snapshot.heartbeat_count);
    let heartbeat_text = str::from_utf8(hb_buf.as_bytes()).unwrap_or("HEARTBEAT ?");

    let mut tick_buf = FixedLogBuffer::<32>::new();
    let _ = write!(&mut tick_buf, "LAST TICK {}", status_snapshot.heartbeat_tick);
    let tick_text = str::from_utf8(tick_buf.as_bytes()).unwrap_or("LAST TICK ?");

    ui::render_status_panel(
        state.chrome.status_handle,
        STATUS_PANEL_WIDTH,
        STATUS_PANEL_HEIGHT,
        "SYSTEM STATUS",
        &[
            (network_text, ui::TEXT_PRIMARY),
            (heartbeat_text, ui::STATUS_OK),
            (tick_text, ui::TEXT_SECONDARY),
            (focus_text, ui::TEXT_SECONDARY),
            (notification_text, ui::TEXT_MUTED),
        ],
    )?;

    state.last_status_snapshot = Some(status_snapshot);
    Ok(())
}

pub(crate) fn refresh_desktop_status(state: &mut DesktopState) -> rt::Result<()> {
    let snapshot = sample_desktop_status(state);
    if state.last_status_snapshot == Some(snapshot) {
        return Ok(());
    }
    render_desktop(state)
}

fn sample_desktop_status(state: &DesktopState) -> DesktopStatusSnapshot {
    let (heartbeat_count, heartbeat_tick) =
        rt::status_snapshot(state.system_status_handle).unwrap_or((0, 0));
    let ipv4_address = rt::network_interface_status(state.network_handle, 0)
        .ok()
        .flatten()
        .map(|info| info.address)
        .unwrap_or(0);
    DesktopStatusSnapshot {
        running_apps: running_app_count(&state.apps) as u32,
        focused_app: state.focused_app,
        heartbeat_count,
        heartbeat_tick,
        ipv4_address,
    }
}

pub(crate) fn sync_cursor(state: &DesktopState) -> rt::Result<()> {
    rt::surface_set_geometry_async(
        state.chrome.cursor_handle,
        state.pointer_x,
        state.pointer_y,
        CURSOR_SIZE,
        CURSOR_SIZE,
        CURSOR_Z_ORDER,
    )
}

fn write_network_status(buffer: &mut FixedLogBuffer<48>, ipv4_address: u32) {
    if ipv4_address == 0 {
        let _ = write!(buffer, "NET OFFLINE");
        return;
    }
    let _ = write!(
        buffer,
        "NET {}.{}.{}.{}",
        (ipv4_address >> 24) & 0xff,
        (ipv4_address >> 16) & 0xff,
        (ipv4_address >> 8) & 0xff,
        ipv4_address & 0xff,
    );
}
