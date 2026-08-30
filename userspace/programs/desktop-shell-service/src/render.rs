use core::{array, fmt::Write, str};

use rt::FixedLogBuffer;
use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;

use crate::{
    CLIPBOARD_HISTORY_LINES, CURSOR_SIZE, CURSOR_Z_ORDER, DesktopState, DesktopStatusSnapshot,
    HISTORY_HEIGHT, HISTORY_WIDTH, LAUNCHER_HEIGHT, LAUNCHER_WIDTH, OVERLAY_RESULT_MAX,
    OverlayMode, PALETTE_BUFFER_BYTES, PALETTE_HEIGHT, PALETTE_WIDTH, STATUS_PANEL_HEIGHT,
    STATUS_PANEL_WIDTH, SWITCHER_HEIGHT, SWITCHER_WIDTH, TOPBAR_HEIGHT, WORKSPACE_COUNT,
    WORKSPACE_OVERVIEW_WIDTH,
    access::{Theme, resolve_theme},
    media::{MEDIA_LINE_COUNT, MEDIA_OVERLAY_HEIGHT, MEDIA_OVERLAY_WIDTH},
    palette_action_label, palette_matches,
    windows::{app_title, launcher_line, running_app_count, overview_tile_rect, sync_focus_shadow},
};

fn theme_of(state: &DesktopState) -> Theme {
    resolve_theme(state.access.high_contrast)
}

/// Panel renderer matching `ui::render_window_state` slot layout but driven by
/// the shell theme so high-contrast recolors every chrome surface.
fn themed_panel(
    state: &DesktopState,
    surface: rt::Handle,
    width: u32,
    height: u32,
    title: &str,
    lines: &[&str],
) -> rt::Result<()> {
    let t = theme_of(state);
    rt::surface_set_rect(
        surface,
        0,
        0,
        0,
        width,
        ui::TITLEBAR_HEIGHT,
        t.titlebar,
        true,
    )?;
    rt::surface_set_rect(
        surface,
        1,
        0,
        ui::TITLEBAR_HEIGHT as i32,
        width,
        height.saturating_sub(ui::TITLEBAR_HEIGHT),
        t.panel,
        true,
    )?;
    rt::surface_set_label(surface, 0, 10, 9, t.text, title)?;
    let close_x = width as i32 - ui::WINDOW_BUTTON_RIGHT_MARGIN - ui::WINDOW_BUTTON_SIZE as i32;
    let minimize_x = close_x - ui::WINDOW_BUTTON_GAP - ui::WINDOW_BUTTON_SIZE as i32;
    let maximize_x = minimize_x - ui::WINDOW_BUTTON_GAP - ui::WINDOW_BUTTON_SIZE as i32;
    rt::surface_set_rect(
        surface,
        5,
        maximize_x,
        ui::WINDOW_BUTTON_TOP,
        ui::WINDOW_BUTTON_SIZE,
        ui::WINDOW_BUTTON_SIZE,
        t.accent,
        true,
    )?;
    rt::surface_set_rect(
        surface,
        2,
        minimize_x,
        ui::WINDOW_BUTTON_TOP,
        ui::WINDOW_BUTTON_SIZE,
        ui::WINDOW_BUTTON_SIZE,
        t.text_muted,
        true,
    )?;
    rt::surface_set_rect(
        surface,
        3,
        close_x,
        ui::WINDOW_BUTTON_TOP,
        ui::WINDOW_BUTTON_SIZE,
        ui::WINDOW_BUTTON_SIZE,
        t.status_warn,
        true,
    )?;
    for (index, line) in lines.iter().copied().enumerate() {
        rt::surface_set_label(
            surface,
            (index + 5) as u32,
            12,
            ui::PANEL_LINE_START_Y + (index as i32 * ui::PANEL_LINE_STEP),
            if index == 0 { t.text } else { t.text_secondary },
            line,
        )?;
    }
    Ok(())
}

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

    themed_panel(
        state,
        state.chrome.topbar_handle,
        state.chrome.output_width,
        TOPBAR_HEIGHT,
        "SERVICEOS DESKTOP",
        &[running_text, focus_text, space_text, notification_text],
    )
}

fn render_shell_chrome(
    state: &mut DesktopState,
    status_snapshot: DesktopStatusSnapshot,
    include_launcher: bool,
) -> rt::Result<()> {
    render_topbar(state, status_snapshot)?;
    if include_launcher {
        render_launcher(state)?;
    }
    render_status_surface(state, status_snapshot)
}

fn render_launcher(state: &mut DesktopState) -> rt::Result<()> {
    let dragging = state.content_drag.is_some();
    let hover = if dragging {
        crate::input::launcher_hover_app(state)
    } else {
        None
    };
    let static_lines: [&str; crate::APP_COUNT] =
        core::array::from_fn(|index| launcher_line(state.apps[index]));
    let mut marked: [FixedLogBuffer<20>; crate::APP_COUNT] =
        core::array::from_fn(|_| FixedLogBuffer::new());
    if dragging {
        for index in 0..crate::APP_COUNT {
            let hovered = hover == Some(state.apps[index].app_id);
            let _ = write!(
                &mut marked[index],
                "{}{}",
                if hovered { "> " } else { "  " },
                static_lines[index]
            );
        }
    }
    let lines: [&str; crate::APP_COUNT] = core::array::from_fn(|index| {
        if dragging {
            marked[index].as_str()
        } else {
            static_lines[index]
        }
    });
    let mut title_buf = FixedLogBuffer::<24>::new();
    let title = if !dragging {
        "APPS"
    } else {
        let _ = write!(
            &mut title_buf,
            "DROP ON {}",
            match hover {
                Some(app_id) => app_title(app_id),
                None => "APP",
            }
        );
        title_buf.as_str()
    };
    let lines_color = if dragging {
        theme_of(state).accent
    } else {
        theme_of(state).text
    };
    let t = theme_of(state);
    rt::surface_set_rect(
        state.chrome.launcher_handle,
        1,
        0,
        ui::TITLEBAR_HEIGHT as i32,
        LAUNCHER_WIDTH,
        LAUNCHER_HEIGHT.saturating_sub(ui::TITLEBAR_HEIGHT),
        t.panel,
        true,
    )?;
    rt::surface_set_rect(
        state.chrome.launcher_handle,
        0,
        0,
        0,
        LAUNCHER_WIDTH,
        ui::TITLEBAR_HEIGHT,
        t.accent_dim,
        true,
    )?;
    rt::surface_set_label(state.chrome.launcher_handle, 0, 10, 9, t.text, title)?;
    for (index, line) in lines.iter().copied().enumerate() {
        rt::surface_set_label(
            state.chrome.launcher_handle,
            (index + 5) as u32,
            12,
            ui::PANEL_LINE_START_Y + (index as i32 * ui::PANEL_LINE_STEP),
            lines_color,
            line,
        )?;
    }
    render_launcher_docs(state)?;
    Ok(())
}

/// Document section under the app grid, on the shared panel line grid:
/// grid row 6 is the "RECENT" header, grid rows 7..10 the up-to-4
/// recency-ranked documents. Rows render only while documents exist;
/// the transition to empty clears the slots exactly once so the
/// app-only layout stays byte-identical (no reserved blank section).
fn render_launcher_docs(state: &mut DesktopState) -> rt::Result<()> {
    let docs_len = state
        .launcher_docs_len
        .min(crate::launcher_docs::LAUNCHER_DOCS_MAX);
    let shown = state.launcher_docs_rendered;
    if docs_len == 0 && shown == 0 {
        return Ok(());
    }
    let t = theme_of(state);
    rt::surface_set_label(
        state.chrome.launcher_handle,
        crate::launcher_docs::DOC_HEADER_SLOT,
        12,
        crate::launcher_docs::doc_header_y(),
        t.text_muted,
        if docs_len == 0 { "" } else { "RECENT" },
    )?;
    for row in 0..crate::launcher_docs::LAUNCHER_DOCS_MAX {
        let mut line = FixedLogBuffer::<56>::new();
        if row < docs_len {
            crate::launcher_docs::doc_row_label(&mut line, &state.launcher_docs[row]);
        }
        rt::surface_set_label(
            state.chrome.launcher_handle,
            crate::launcher_docs::DOC_ROW_SLOT_BASE + row as u32,
            12,
            crate::launcher_docs::doc_row_y(row),
            t.text_secondary,
            line.as_str(),
        )?;
    }
    state.launcher_docs_rendered = docs_len;
    Ok(())
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

    let t = theme_of(state);
    themed_status_panel(
        state.chrome.status_handle,
        STATUS_PANEL_WIDTH,
        STATUS_PANEL_HEIGHT,
        "SYSTEM STATUS",
        &[
            (network_text, t.text),
            ("STATUS STEADY", t.status_ok),
            (service_text, t.text_secondary),
            (space_text, t.text_secondary),
            (notif_text, t.text_muted),
            (focus_text, t.text_secondary),
        ],
        t,
    )
}

/// Status-panel layout mirroring `ui::render_status_panel` with theme colors.
fn themed_status_panel(
    surface: rt::Handle,
    width: u32,
    height: u32,
    title: &str,
    lines: &[(&str, u32)],
    t: Theme,
) -> rt::Result<()> {
    rt::surface_set_rect(
        surface,
        7,
        0,
        24,
        width,
        height.saturating_sub(24),
        t.panel,
        true,
    )?;
    rt::surface_set_rect(surface, 0, 0, 0, width, 24, t.accent_dim, true)?;
    rt::surface_set_label(surface, 0, 8, 8, t.text, title)?;
    for (index, (line, color)) in lines.iter().copied().enumerate() {
        rt::surface_set_label(
            surface,
            (index + 1) as u32,
            10,
            34 + (index as i32 * ui::PANEL_LINE_STEP),
            color,
            line,
        )?;
    }
    Ok(())
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
    let show_workspace = state.overlay_mode == OverlayMode::WorkspaceOverview;

    rt::surface_set_visibility(state.chrome.switcher_handle, show_switcher)?;
    rt::surface_set_visibility(state.chrome.palette_handle, show_palette)?;
    rt::surface_set_visibility(state.chrome.notifications_handle, show_notifications)?;
    rt::surface_set_visibility(state.chrome.clipboard_handle, show_clipboard)?;
    rt::surface_set_visibility(state.chrome.media_handle, show_media)?;
    rt::surface_set_visibility(state.chrome.workspace_handle, show_workspace)?;

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
    if show_workspace {
        render_workspace_overlay(state)?;
    }
    Ok(())
}

/// Mission-control style workspace grid: one tile per workspace with its
/// window count and active marker; selection follows overlay_selection.
fn render_workspace_overlay(state: &DesktopState) -> rt::Result<()> {
    let surface = state.chrome.workspace_handle;
    let t = theme_of(state);
    rt::surface_set_fill(surface, t.panel)?;
    rt::surface_clear_scene(surface)?;
    rt::surface_set_rect(
        surface,
        0,
        0,
        0,
        WORKSPACE_OVERVIEW_WIDTH,
        ui::TITLEBAR_HEIGHT,
        t.accent_dim,
        true,
    )?;
    rt::surface_set_label(surface, 0, 10, 9, t.text, "WORKSPACES")?;

    for index in 0..WORKSPACE_COUNT as usize {
        let workspace_id = index as u32 + 1;
        let mut windows = 0usize;
        for slot in state.apps.iter().copied() {
            if slot.running && slot.workspace_id == workspace_id {
                windows += 1;
            }
        }
        let selected = index == state.overlay_selection.min(WORKSPACE_COUNT as usize - 1);
        let (tx, ty, tw, th) = overview_tile_rect(index);
        rt::surface_set_rect(
            surface,
            index as u32 + 1,
            tx,
            ty,
            tw,
            th,
            if selected { t.accent } else { t.window_alt },
            true,
        )?;
        let mut title_buf: FixedLogBuffer<16> = FixedLogBuffer::new();
        let _ = write!(&mut title_buf, "WS {}", workspace_id);
        let mut sub_buf: FixedLogBuffer<24> = FixedLogBuffer::new();
        let _ = write!(
            &mut sub_buf,
            "{} WIN{}",
            windows,
            if state.active_workspace == workspace_id { " *ACTIVE" } else { "" }
        );
        rt::surface_set_label(
            surface,
            index as u32 * 2 + 1,
            tx + 10,
            ty + th as i32 / 2 - 14,
            if selected { t.text } else { t.text_muted },
            title_buf.as_str(),
        )?;
        rt::surface_set_label(
            surface,
            index as u32 * 2 + 2,
            tx + 10,
            ty + th as i32 / 2 + 8,
            if selected { t.text } else { t.text_secondary },
            sub_buf.as_str(),
        )?;
    }
    Ok(())
}

fn render_switcher_overlay(state: &DesktopState) -> rt::Result<()> {
    let model = crate::switcher::switcher_model(state);
    let surface = state.chrome.switcher_handle;
    let t = theme_of(state);
    rt::surface_set_fill(surface, t.panel)?;
    rt::surface_clear_scene(surface)?;
    rt::surface_set_rect(
        surface,
        0,
        0,
        0,
        SWITCHER_WIDTH,
        ui::TITLEBAR_HEIGHT,
        t.accent_dim,
        true,
    )?;
    rt::surface_set_label(surface, 0, 10, 9, t.text, "TASK SWITCHER")?;

    if model.count == 0 {
        rt::surface_set_label(
            surface,
            1,
            12,
            ui::PANEL_LINE_START_Y,
            t.text_secondary,
            "NO VISIBLE WINDOWS",
        )?;
        return Ok(());
    }

    for index in 0..model.count {
        let selected = index == state.switcher_selection % model.count;
        let tile_x = crate::SWITCHER_TILE_PAD
            + index as i32 * (crate::SWITCHER_TILE_WIDTH as i32 + crate::SWITCHER_TILE_PAD);
        let tile_y = ui::TITLEBAR_HEIGHT as i32 + 10;
        rt::surface_set_rect(
            surface,
            (index * 2 + 1) as u32,
            tile_x,
            tile_y,
            crate::SWITCHER_TILE_WIDTH,
            crate::SWITCHER_TILE_HEIGHT,
            if selected { t.accent } else { t.window_alt },
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
            if selected { t.text } else { t.text_muted },
            initial_buf.as_str(),
        )?;
    }

    let mut footer: FixedLogBuffer<48> = FixedLogBuffer::new();
    let selected_title = app_title(model.candidates[state.switcher_selection % model.count]);
    let _ = write!(
        &mut footer,
        "{} · TAB CYCLES · ALT RELEASE COMMITS",
        selected_title
    );
    rt::surface_set_label(
        surface,
        11,
        12,
        SWITCHER_HEIGHT as i32 - 18,
        t.text_secondary,
        footer.as_str(),
    )
}

fn render_palette_overlay(state: &mut DesktopState) -> rt::Result<()> {
    let mut results =
        [crate::PaletteEntry::Action(crate::PaletteAction::ShowNotifications); OVERLAY_RESULT_MAX];
    let count = palette_matches(state, &mut results);
    let query = str::from_utf8(&state.palette_query[..state.palette_query_len]).unwrap_or("");
    let t = theme_of(state);
    let (buffer_slot, buffer) = state.palette_buffers.advance();
    let bytes = &mut buffer.as_slice_mut()[..PALETTE_BUFFER_BYTES];
    ui::draw_window_frame_rgba8888(
        bytes,
        PALETTE_WIDTH as usize,
        PALETTE_WIDTH as usize,
        PALETTE_HEIGHT as usize,
        true,
        t.panel,
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
        t.text,
        line0.as_str(),
    );

    if count == 0 {
        rt::draw_text_rgba8888(
            bytes,
            PALETTE_WIDTH as usize,
            12,
            56,
            t.text_secondary,
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
            let _ = write!(&mut line, "{}", prefix);
            match results[index] {
                crate::PaletteEntry::Action(action) => {
                    let _ = write!(&mut line, "{}", palette_action_label(action));
                }
                crate::PaletteEntry::Doc(doc_hit) => {
                    let path = doc_hit.path_str();
                    let icon = crate::palette_docs::doc_kind_icon(doc_hit.kind);
                    if doc_hit.line != 0 {
                        let _ = write!(&mut line, "{} {} :{}", icon, path, doc_hit.line);
                    } else {
                        let _ = write!(&mut line, "{} {}", icon, path);
                    }
                }
            }
            rt::draw_text_rgba8888(
                bytes,
                PALETTE_WIDTH as usize,
                12,
                56 + (index as i32 * ui::PANEL_LINE_STEP),
                if index == state.overlay_selection {
                    t.text
                } else {
                    t.text_secondary
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
    let selected_reopenable = state
        .notification_history
        .get(state.overlay_selection.min(crate::NOTIFICATION_HISTORY_MAX - 1))
        .is_some_and(|entry| {
            entry.occupied && entry.reopenable && state.notification_history_len != 0
        });
    let footer = if selected_reopenable {
        "[R] REOPEN  [D] DISMISS  [A] DISMISS ALL"
    } else {
        "[A] DISMISS ALL   [F] FOCUS SOURCE"
    };
    let _ = write!(&mut lines[count], "{footer}");
    render_overlay_panel(
        state,
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
            state,
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
        state,
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
        state,
        state.chrome.media_handle,
        MEDIA_OVERLAY_WIDTH,
        MEDIA_OVERLAY_HEIGHT,
        "MEDIA",
        &lines[..count],
    )
}

fn render_overlay_panel<const N: usize>(
    state: &DesktopState,
    surface: rt::Handle,
    width: u32,
    height: u32,
    title: &str,
    lines: &[FixedLogBuffer<N>],
) -> rt::Result<()> {
    let t = theme_of(state);
    rt::surface_set_fill(surface, t.panel)?;
    rt::surface_clear_scene(surface)?;
    rt::surface_set_rect(
        surface,
        0,
        0,
        0,
        width,
        ui::TITLEBAR_HEIGHT,
        t.accent_dim,
        true,
    )?;
    rt::surface_set_label(surface, 0, 10, 9, t.text, title)?;
    for (index, line) in lines.iter().enumerate() {
        rt::surface_set_label(
            surface,
            (index + 1) as u32,
            12,
            ui::PANEL_LINE_START_Y + (index as i32 * ui::PANEL_LINE_STEP),
            if index == 0 { t.text } else { t.text_secondary },
            line.as_str(),
        )?;
    }
    let _ = height;
    Ok(())
}
