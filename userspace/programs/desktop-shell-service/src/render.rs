use core::{array, fmt::Write, str};

use rt::FixedLogBuffer;
use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;

use crate::{
    CLIPBOARD_HISTORY_LINES, CURSOR_SIZE, CURSOR_Z_ORDER, DesktopState, DesktopStatusSnapshot,
    HISTORY_HEIGHT, HISTORY_WIDTH, LAUNCHER_HEIGHT, LAUNCHER_WIDTH, OVERLAY_RESULT_MAX,
    OverlayMode, PALETTE_BUFFER_BYTES, PALETTE_HEIGHT, PALETTE_WIDTH, STATUS_PANEL_HEIGHT,
    STATUS_PANEL_WIDTH, SWITCHER_HEIGHT, SWITCHER_WIDTH, TOPBAR_HEIGHT, WORKSPACE_COUNT,
    media::{MEDIA_LINE_COUNT, MEDIA_OVERLAY_HEIGHT, MEDIA_OVERLAY_WIDTH},
    palette_action_label, palette_matches,
    windows::{
        app_title, launcher_line, running_app_count, sync_focus_shadow,
    },
};

pub(crate) fn render_desktop(state: &mut DesktopState) -> rt::Result<()> {
    let status_snapshot = snapshot_for_render(state);
    render_shell_chrome(state, status_snapshot, true)?;
    sync_focus_shadow(state);
    render_overlays(state)?;
    state.last_status_snapshot = Some(status_snapshot);
    Ok(())
}

pub(crate) fn render_focus_chrome(state: &mut DesktopState) -> rt::Result<()> {
    let status_snapshot = snapshot_for_render(state);
    render_shell_chrome(state, status_snapshot, false)?;
    sync_focus_shadow(state);
    state.last_status_snapshot = Some(status_snapshot);
    Ok(())
}

pub(crate) fn refresh_desktop_status(state: &mut DesktopState) -> rt::Result<()> {
    let snapshot = sample_desktop_status(state);
    if state.last_status_snapshot == Some(snapshot) {
        return Ok(());
    }
    render_status_surface(state, snapshot)?;
    state.last_status_snapshot = Some(snapshot);
    Ok(())
}

pub(crate) fn render_overlays_only(state: &mut DesktopState) -> rt::Result<()> {
    render_overlays(state)
}

fn render_topbar(state: &DesktopState, status_snapshot: DesktopStatusSnapshot) -> rt::Result<()> {
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

    let mut space_buf = FixedLogBuffer::<32>::new();
    let _ = write!(
        &mut space_buf,
        "SPACE {}/{}",
        status_snapshot.active_workspace, WORKSPACE_COUNT
    );
    let space_text = str::from_utf8(space_buf.as_bytes()).unwrap_or("SPACE ?");

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
        &[running_text, focus_text, space_text, notification_text],
    )
}

fn render_shell_chrome(
    state: &DesktopState,
    status_snapshot: DesktopStatusSnapshot,
    include_launcher: bool,
) -> rt::Result<()> {
    render_topbar(state, status_snapshot)?;
    if include_launcher {
        render_launcher(state)?;
    }
    render_status_surface(state, status_snapshot)
}

fn render_launcher(state: &DesktopState) -> rt::Result<()> {
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
    )
}

fn render_status_surface(
    state: &DesktopState,
    status_snapshot: DesktopStatusSnapshot,
) -> rt::Result<()> {
    let mut space_buf = FixedLogBuffer::<32>::new();
    let _ = write!(
        &mut space_buf,
        "SPACE {}/{}",
        status_snapshot.active_workspace, WORKSPACE_COUNT
    );
    let space_text = str::from_utf8(space_buf.as_bytes()).unwrap_or("SPACE ?");

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

    let mut service_buf = FixedLogBuffer::<32>::new();
    let _ = write!(
        &mut service_buf,
        "SERVICES {}",
        status_snapshot.tracked_services
    );
    let service_text = str::from_utf8(service_buf.as_bytes()).unwrap_or("SERVICES ?");

    let mut notif_buf = FixedLogBuffer::<32>::new();
    let _ = write!(
        &mut notif_buf,
        "NOTICES {}",
        status_snapshot.notification_count
    );
    let notif_text = str::from_utf8(notif_buf.as_bytes()).unwrap_or("NOTICES ?");

    ui::render_status_panel(
        state.chrome.status_handle,
        STATUS_PANEL_WIDTH,
        STATUS_PANEL_HEIGHT,
        "SYSTEM STATUS",
        &[
            (network_text, ui::TEXT_PRIMARY),
            ("STATUS STEADY", ui::STATUS_OK),
            (service_text, ui::TEXT_SECONDARY),
            (space_text, ui::TEXT_SECONDARY),
            (notif_text, ui::TEXT_MUTED),
            (focus_text, ui::TEXT_SECONDARY),
        ],
    )
}

fn sample_desktop_status(state: &DesktopState) -> DesktopStatusSnapshot {
    let (_, _, tracked_services) =
        rt::status_snapshot(state.system_status_handle).unwrap_or((0, 0, 0));
    let ipv4_address = rt::network_interface_status(state.network_handle, 0)
        .ok()
        .flatten()
        .map(|info| info.address)
        .unwrap_or(0);
    DesktopStatusSnapshot {
        running_apps: running_app_count(&state.apps) as u32,
        focused_app: state.focused_app,
        active_workspace: state.active_workspace,
        tracked_services,
        ipv4_address,
        notification_count: state.notification_history_len as u32,
    }
}

fn snapshot_for_render(state: &DesktopState) -> DesktopStatusSnapshot {
    let mut snapshot = state
        .last_status_snapshot
        .unwrap_or_else(|| sample_desktop_status(state));
    snapshot.running_apps = running_app_count(&state.apps) as u32;
    snapshot.focused_app = state.focused_app;
    snapshot.active_workspace = state.active_workspace;
    snapshot.notification_count = state.notification_history_len as u32;
    snapshot
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

fn render_overlays(state: &mut DesktopState) -> rt::Result<()> {
    let show_switcher = state.overlay_mode == OverlayMode::Switcher;
    let show_palette = state.overlay_mode == OverlayMode::CommandPalette;
    let show_notifications = state.overlay_mode == OverlayMode::Notifications;
    let show_clipboard = state.overlay_mode == OverlayMode::ClipboardHistory;
    let show_media = state.overlay_mode == OverlayMode::Media;

    rt::surface_set_visibility(state.chrome.switcher_handle, show_switcher)?;
    rt::surface_set_visibility(state.chrome.palette_handle, show_palette)?;
    rt::surface_set_visibility(state.chrome.notifications_handle, show_notifications)?;
    rt::surface_set_visibility(state.chrome.clipboard_handle, show_clipboard)?;
    rt::surface_set_visibility(state.chrome.media_handle, show_media)?;

    if show_switcher {
        render_switcher_overlay(state)?;
    }
    if show_palette {
        render_palette_overlay(state)?;
    }
    if show_notifications {
        render_notification_overlay(state)?;
    }
    if show_clipboard {
        render_clipboard_overlay(state)?;
    }
    if show_media {
        render_media_overlay(state)?;
    }
    Ok(())
}

fn render_switcher_overlay(state: &DesktopState) -> rt::Result<()> {
    let model = crate::switcher::switcher_model(state);
    let surface = state.chrome.switcher_handle;
    rt::surface_set_fill(surface, ui::BG_PANEL)?;
    rt::surface_clear_scene(surface)?;
    rt::surface_set_rect(
        surface,
        0,
        0,
        0,
        SWITCHER_WIDTH,
        ui::TITLEBAR_HEIGHT,
        ui::ACCENT_DIM,
        true,
    )?;
    rt::surface_set_label(surface, 0, 10, 9, ui::TEXT_PRIMARY, "TASK SWITCHER")?;

    if model.count == 0 {
        rt::surface_set_label(
            surface,
            1,
            12,
            ui::PANEL_LINE_START_Y,
            ui::TEXT_SECONDARY,
            "NO VISIBLE WINDOWS",
        )?;
        return Ok(());
    }

    for index in 0..model.count {
        let selected = index == state.switcher_selection % model.count;
        let tile_x =
            crate::SWITCHER_TILE_PAD + index as i32 * (crate::SWITCHER_TILE_WIDTH as i32 + crate::SWITCHER_TILE_PAD);
        let tile_y = ui::TITLEBAR_HEIGHT as i32 + 10;
        rt::surface_set_rect(
            surface,
            (index * 2 + 1) as u32,
            tile_x,
            tile_y,
            crate::SWITCHER_TILE_WIDTH,
            crate::SWITCHER_TILE_HEIGHT,
            if selected {
                ui::ACCENT_DIM
            } else {
                ui::BG_WINDOW_ALT
            },
            true,
        )?;
        let title = app_title(model.candidates[index]);
        let initial = title.as_bytes().first().copied().unwrap_or(b'?');
        let mut initial_buf: FixedLogBuffer<4> = FixedLogBuffer::new();
        let _ = write!(&mut initial_buf, "{}", initial as char);
        rt::surface_set_label(
            surface,
            (index * 2 + 2) as u32,
            tile_x + crate::SWITCHER_TILE_WIDTH as i32 / 2 - 6,
            tile_y + crate::SWITCHER_TILE_HEIGHT as i32 / 2 - 8,
            if selected { ui::TEXT_PRIMARY } else { ui::TEXT_MUTED },
            initial_buf.as_str(),
        )?;
    }

    let mut footer: FixedLogBuffer<48> = FixedLogBuffer::new();
    let selected_title = app_title(model.candidates[state.switcher_selection % model.count]);
    let _ = write!(&mut footer, "{} · TAB CYCLES · ALT RELEASE COMMITS", selected_title);
    rt::surface_set_label(
        surface,
        11,
        12,
        SWITCHER_HEIGHT as i32 - 18,
        ui::TEXT_SECONDARY,
        footer.as_str(),
    )
}

fn render_palette_overlay(state: &mut DesktopState) -> rt::Result<()> {
    let mut results = [crate::PaletteAction::ShowNotifications; OVERLAY_RESULT_MAX];
    let count = palette_matches(state, &mut results);
    let query = str::from_utf8(&state.palette_query[..state.palette_query_len]).unwrap_or("");
    let (buffer_slot, buffer) = state.palette_buffers.advance();
    let bytes = &mut buffer.as_slice_mut()[..PALETTE_BUFFER_BYTES];
    ui::draw_window_frame_rgba8888(
        bytes,
        PALETTE_WIDTH as usize,
        PALETTE_WIDTH as usize,
        PALETTE_HEIGHT as usize,
        true,
        ui::BG_PANEL,
        "COMMAND PALETTE",
    );

    let mut line0 = FixedLogBuffer::<64>::new();
    let _ = write!(
        &mut line0,
        "QUERY {}",
        if query.is_empty() {
            "TYPE TO SEARCH"
        } else {
            query
        }
    );
    rt::draw_text_rgba8888(
        bytes,
        PALETTE_WIDTH as usize,
        12,
        42,
        ui::TEXT_PRIMARY,
        line0.as_str(),
    );

    if count == 0 {
        rt::draw_text_rgba8888(
            bytes,
            PALETTE_WIDTH as usize,
            12,
            56,
            ui::TEXT_SECONDARY,
            "NO MATCHES",
        );
    } else {
        for index in 0..count {
            let prefix = if index == state.overlay_selection {
                "> "
            } else {
                "  "
            };
            let mut line = FixedLogBuffer::<64>::new();
            let _ = write!(
                &mut line,
                "{}{}",
                prefix,
                palette_action_label(results[index])
            );
            rt::draw_text_rgba8888(
                bytes,
                PALETTE_WIDTH as usize,
                12,
                56 + (index as i32 * ui::PANEL_LINE_STEP),
                if index == state.overlay_selection {
                    ui::TEXT_PRIMARY
                } else {
                    ui::TEXT_SECONDARY
                },
                line.as_str(),
            );
        }
    }

    state
        .palette_presenter
        .present(buffer_slot, PALETTE_WIDTH, PALETTE_HEIGHT)
}

fn render_notification_overlay(state: &DesktopState) -> rt::Result<()> {
    let mut lines: [FixedLogBuffer<80>; 7] = array::from_fn(|_| FixedLogBuffer::new());
    let mut count = 0usize;
    for entry in state
        .notification_history
        .iter()
        .copied()
        .take(state.notification_history_len)
        .filter(|entry| entry.occupied)
    {
        if count == 6 {
            break;
        }
        let prefix = if count == state.overlay_selection {
            "> "
        } else {
            "  "
        };
        let text = str::from_utf8(&entry.text[..entry.text_len]).unwrap_or("NOTICE");
        let _ = write!(&mut lines[count], "{}{}", prefix, text);
        count += 1;
    }
    if count == 0 {
        let _ = write!(&mut lines[0], "NO NOTIFICATIONS");
        count = 1;
    }
    let _ = write!(&mut lines[count], "[A] DISMISS ALL   [F] FOCUS SOURCE");
    render_overlay_panel(
        state.chrome.notifications_handle,
        HISTORY_WIDTH,
        HISTORY_HEIGHT,
        "NOTIFICATION HISTORY",
        &lines[..count + 1],
    )
}

fn render_clipboard_overlay(state: &DesktopState) -> rt::Result<()> {
    let mut lines: [FixedLogBuffer<80>; CLIPBOARD_HISTORY_LINES] =
        array::from_fn(|_| FixedLogBuffer::new());
    if state.clipboard_service_handle == rt::INVALID_HANDLE {
        let _ = write!(&mut lines[0], "CLIPBOARD UNAVAILABLE");
        return render_overlay_panel(
            state.chrome.clipboard_handle,
            HISTORY_WIDTH,
            HISTORY_HEIGHT,
            "CLIPBOARD HISTORY",
            &lines[..1],
        );
    }
    let mut count = 0usize;
    for index in 0..CLIPBOARD_HISTORY_LINES {
        match rt::clipboard_history_entry(state.clipboard_service_handle, index as u32) {
            Ok(entry) => {
                let prefix = if index == state.overlay_selection {
                    "> "
                } else {
                    "  "
                };
                let text = str::from_utf8(&entry.bytes[..entry.len as usize]).unwrap_or("CLIP");
                let _ = write!(&mut lines[count], "{}{}", prefix, text);
                count += 1;
            }
            Err(rt::Error::NotFound) => break,
            Err(_) => {
                let _ = write!(&mut lines[0], "CLIPBOARD UNAVAILABLE");
                count = 1;
                break;
            }
        }
    }
    if count == 0 {
        let _ = write!(&mut lines[0], "CLIPBOARD EMPTY");
        count = 1;
    }
    render_overlay_panel(
        state.chrome.clipboard_handle,
        HISTORY_WIDTH,
        HISTORY_HEIGHT,
        "CLIPBOARD HISTORY",
        &lines[..count],
    )
}

fn render_media_overlay(state: &mut DesktopState) -> rt::Result<()> {
    let snapshot = crate::media::sample_media(state.audio_service_handle);
    let mut lines: [FixedLogBuffer<48>; MEDIA_LINE_COUNT] =
        array::from_fn(|_| FixedLogBuffer::new());
    let count = crate::media::write_media_lines(
        &snapshot,
        state.master_volume,
        state.master_muted,
        &mut lines,
    );
    render_overlay_panel(
        state.chrome.media_handle,
        MEDIA_OVERLAY_WIDTH,
        MEDIA_OVERLAY_HEIGHT,
        "MEDIA",
        &lines[..count],
    )
}

fn render_overlay_panel<const N: usize>(
    surface: rt::Handle,
    width: u32,
    height: u32,
    title: &str,
    lines: &[FixedLogBuffer<N>],
) -> rt::Result<()> {
    rt::surface_set_fill(surface, ui::BG_PANEL)?;
    rt::surface_clear_scene(surface)?;
    rt::surface_set_rect(
        surface,
        0,
        0,
        0,
        width,
        ui::TITLEBAR_HEIGHT,
        ui::ACCENT_DIM,
        true,
    )?;
    rt::surface_set_label(surface, 0, 10, 9, ui::TEXT_PRIMARY, title)?;
    for (index, line) in lines.iter().enumerate() {
        rt::surface_set_label(
            surface,
            (index + 1) as u32,
            12,
            ui::PANEL_LINE_START_Y + (index as i32 * ui::PANEL_LINE_STEP),
            if index == 0 {
                ui::TEXT_PRIMARY
            } else {
                ui::TEXT_SECONDARY
            },
            line.as_str(),
        )?;
    }
    let _ = height;
    Ok(())
}
