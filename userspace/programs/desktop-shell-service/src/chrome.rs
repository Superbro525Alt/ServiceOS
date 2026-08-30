use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;

use crate::media::{MEDIA_OVERLAY_HEIGHT, MEDIA_OVERLAY_WIDTH};
use crate::state::{
    CURSOR_SIZE, CURSOR_Z_ORDER, Chrome, HISTORY_HEIGHT, HISTORY_WIDTH, LAUNCHER_WIDTH,
    PALETTE_HEIGHT, PALETTE_WIDTH, PANEL_MARGIN, SESSION_ID, STATUS_PANEL_HEIGHT,
    STATUS_PANEL_WIDTH, SWITCHER_HEIGHT, SWITCHER_WIDTH, TOPBAR_HEIGHT,
};

pub(crate) fn create_chrome(
    graphics_handle: rt::Handle,
    output_width: u32,
    output_height: u32,
    high_contrast: bool,
) -> rt::Result<Chrome> {
    let theme = crate::access::resolve_theme(high_contrast);
    let (_, desktop_handle) = rt::graphics_surface_create(
        graphics_handle,
        SESSION_ID,
        0,
        0,
        output_width,
        output_height,
        0,
        theme.desktop_bg,
        false,
    )?;
    let (_, topbar_handle) = rt::graphics_surface_create(
        graphics_handle,
        SESSION_ID,
        0,
        0,
        output_width,
        TOPBAR_HEIGHT,
        1,
        theme.panel,
        false,
    )?;
    let launcher_x = PANEL_MARGIN as i32;
    let launcher_y = (TOPBAR_HEIGHT + PANEL_MARGIN) as i32;
    let (_, launcher_handle) = rt::graphics_surface_create(
        graphics_handle,
        SESSION_ID,
        launcher_x,
        launcher_y,
        LAUNCHER_WIDTH,
        264,
        2,
        theme.panel,
        false,
    )?;
    let status_x = (output_width.saturating_sub(STATUS_PANEL_WIDTH + PANEL_MARGIN)) as i32;
    let status_y = (TOPBAR_HEIGHT + PANEL_MARGIN) as i32;
    let (_, status_handle) = rt::graphics_surface_create(
        graphics_handle,
        SESSION_ID,
        status_x,
        status_y,
        STATUS_PANEL_WIDTH,
        STATUS_PANEL_HEIGHT,
        2,
        theme.panel,
        false,
    )?;
    let (_, switcher_handle) = rt::graphics_surface_create(
        graphics_handle,
        SESSION_ID,
        ((output_width.saturating_sub(SWITCHER_WIDTH)) / 2) as i32,
        ((output_height.saturating_sub(SWITCHER_HEIGHT)) / 2) as i32,
        SWITCHER_WIDTH,
        SWITCHER_HEIGHT,
        CURSOR_Z_ORDER - 3,
        theme.panel,
        false,
    )?;
    let (_, palette_handle) = rt::graphics_surface_create(
        graphics_handle,
        SESSION_ID,
        ((output_width.saturating_sub(PALETTE_WIDTH)) / 2) as i32,
        72,
        PALETTE_WIDTH,
        PALETTE_HEIGHT,
        CURSOR_Z_ORDER - 3,
        theme.panel,
        false,
    )?;
    let (_, notifications_handle) = rt::graphics_surface_create(
        graphics_handle,
        SESSION_ID,
        (output_width.saturating_sub(HISTORY_WIDTH + PANEL_MARGIN)) as i32,
        (TOPBAR_HEIGHT + PANEL_MARGIN + STATUS_PANEL_HEIGHT + 12) as i32,
        HISTORY_WIDTH,
        HISTORY_HEIGHT,
        CURSOR_Z_ORDER - 3,
        theme.panel,
        false,
    )?;
    let (_, clipboard_handle) = rt::graphics_surface_create(
        graphics_handle,
        SESSION_ID,
        (output_width.saturating_sub(HISTORY_WIDTH + PANEL_MARGIN)) as i32,
        (TOPBAR_HEIGHT + PANEL_MARGIN) as i32,
        HISTORY_WIDTH,
        HISTORY_HEIGHT,
        CURSOR_Z_ORDER - 3,
        theme.panel,
        false,
    )?;
    let (_, media_handle) = rt::graphics_surface_create(
        graphics_handle,
        SESSION_ID,
        ((output_width.saturating_sub(MEDIA_OVERLAY_WIDTH)) / 2) as i32,
        72,
        MEDIA_OVERLAY_WIDTH,
        MEDIA_OVERLAY_HEIGHT,
        CURSOR_Z_ORDER - 3,
        theme.panel,
        false,
    )?;
    let (_, gesture_handle) = rt::graphics_surface_create(
        graphics_handle,
        SESSION_ID,
        0,
        0,
        output_width,
        output_height,
        CURSOR_Z_ORDER - 2,
        0,
        false,
    )?;
    let (_, workspace_handle) = rt::graphics_surface_create(
        graphics_handle,
        SESSION_ID,
        ((output_width.saturating_sub(crate::WORKSPACE_OVERVIEW_WIDTH)) / 2) as i32,
        ((output_height.saturating_sub(crate::WORKSPACE_OVERVIEW_HEIGHT)) / 2) as i32,
        crate::WORKSPACE_OVERVIEW_WIDTH,
        crate::WORKSPACE_OVERVIEW_HEIGHT,
        CURSOR_Z_ORDER - 3,
        theme.panel,
        false,
    )?;
    let (_, login_handle) = rt::graphics_surface_create(
        graphics_handle,
        SESSION_ID,
        ((output_width.saturating_sub(crate::LOGIN_WIDTH)) / 2) as i32,
        ((output_height.saturating_sub(crate::LOGIN_HEIGHT)) / 2) as i32,
        crate::LOGIN_WIDTH,
        crate::LOGIN_HEIGHT,
        CURSOR_Z_ORDER - 3,
        theme.panel,
        false,
    )?;
    let (_, cursor_handle) = rt::graphics_surface_create(
        graphics_handle,
        SESSION_ID,
        (output_width / 2) as i32,
        (output_height / 2) as i32,
        CURSOR_SIZE,
        CURSOR_SIZE,
        CURSOR_Z_ORDER,
        0,
        false,
    )?;
    ui::render_cursor(cursor_handle, CURSOR_SIZE)?;

    Ok(Chrome {
        desktop_handle,
        topbar_handle,
        launcher_handle,
        status_handle,
        switcher_handle,
        palette_handle,
        notifications_handle,
        clipboard_handle,
        media_handle,
        gesture_handle,
        workspace_handle,
        login_handle,
        cursor_handle,
        output_width,
        output_height,
        zoom_panels: [
            (
                launcher_handle,
                launcher_x,
                launcher_y,
                LAUNCHER_WIDTH,
                264,
                2,
            ),
            (
                status_handle,
                status_x,
                status_y,
                STATUS_PANEL_WIDTH,
                STATUS_PANEL_HEIGHT,
                2,
            ),
        ],
    })
}

/// Screen-space rect (x, y, width, height) of an overlay surface, mirroring
/// the geometry chosen in `create_chrome`.
pub(crate) fn overlay_rect(
    chrome: &Chrome,
    mode: crate::OverlayMode,
) -> Option<(i32, i32, i32, i32)> {
    let output_width = chrome.output_width;
    let output_height = chrome.output_height;
    match mode {
        crate::OverlayMode::None => None,
        crate::OverlayMode::Switcher => Some((
            ((output_width.saturating_sub(crate::SWITCHER_WIDTH)) / 2) as i32,
            ((output_height.saturating_sub(crate::SWITCHER_HEIGHT)) / 2) as i32,
            crate::SWITCHER_WIDTH as i32,
            crate::SWITCHER_HEIGHT as i32,
        )),
        crate::OverlayMode::CommandPalette => Some((
            ((output_width.saturating_sub(crate::PALETTE_WIDTH)) / 2) as i32,
            72,
            crate::PALETTE_WIDTH as i32,
            crate::PALETTE_HEIGHT as i32,
        )),
        crate::OverlayMode::Notifications => Some((
            output_width.saturating_sub(crate::HISTORY_WIDTH + PANEL_MARGIN) as i32,
            TOPBAR_HEIGHT as i32 + PANEL_MARGIN as i32 + crate::STATUS_PANEL_HEIGHT as i32 + 12,
            crate::HISTORY_WIDTH as i32,
            crate::HISTORY_HEIGHT as i32,
        )),
        crate::OverlayMode::ClipboardHistory => Some((
            output_width.saturating_sub(crate::HISTORY_WIDTH + PANEL_MARGIN) as i32,
            TOPBAR_HEIGHT as i32 + PANEL_MARGIN as i32,
            crate::HISTORY_WIDTH as i32,
            crate::HISTORY_HEIGHT as i32,
        )),
        crate::OverlayMode::Media => Some((
            ((output_width.saturating_sub(MEDIA_OVERLAY_WIDTH)) / 2) as i32,
            72,
            MEDIA_OVERLAY_WIDTH as i32,
            MEDIA_OVERLAY_HEIGHT as i32,
        )),
        crate::OverlayMode::WorkspaceOverview => Some((
            ((output_width.saturating_sub(crate::WORKSPACE_OVERVIEW_WIDTH)) / 2) as i32,
            ((output_height.saturating_sub(crate::WORKSPACE_OVERVIEW_HEIGHT)) / 2) as i32,
            crate::WORKSPACE_OVERVIEW_WIDTH as i32,
            crate::WORKSPACE_OVERVIEW_HEIGHT as i32,
        )),
        crate::OverlayMode::Login => Some((
            ((output_width.saturating_sub(crate::LOGIN_WIDTH)) / 2) as i32,
            ((output_height.saturating_sub(crate::LOGIN_HEIGHT)) / 2) as i32,
            crate::LOGIN_WIDTH as i32,
            crate::LOGIN_HEIGHT as i32,
        )),
    }
}

/// Local row index under an overlay panel's line layout, if any.
pub(crate) fn overlay_row_at(local_y: i32, max_rows: usize) -> Option<usize> {
    for index in 0..max_rows {
        let line_y = ui::PANEL_LINE_START_Y + (index as i32 * ui::PANEL_LINE_STEP);
        if local_y >= line_y - 2 && local_y < line_y - 2 + ui::PANEL_LINE_STEP {
            return Some(index);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CLIPBOARD_HISTORY_LINES, OverlayMode};

    fn chrome() -> Chrome {
        Chrome {
            desktop_handle: 0,
            topbar_handle: 0,
            launcher_handle: 0,
            status_handle: 0,
            switcher_handle: 0,
            palette_handle: 0,
            notifications_handle: 0,
            clipboard_handle: 0,
            media_handle: 0,
            gesture_handle: 0,
            workspace_handle: 0,
            login_handle: 0,
            cursor_handle: 0,
            output_width: 1280,
            output_height: 800,
            zoom_panels: [
                (
                    0,
                    PANEL_MARGIN as i32,
                    (TOPBAR_HEIGHT + PANEL_MARGIN) as i32,
                    LAUNCHER_WIDTH,
                    264,
                    2,
                ),
                (
                    0,
                    (1280u32.saturating_sub(STATUS_PANEL_WIDTH + PANEL_MARGIN)) as i32,
                    (TOPBAR_HEIGHT + PANEL_MARGIN) as i32,
                    STATUS_PANEL_WIDTH,
                    STATUS_PANEL_HEIGHT,
                    2,
                ),
            ],
        }
    }

    #[test]
    fn overlay_rects_match_chrome_geometry() {
        let chrome = chrome();
        let (sx, sy, sw, sh) = overlay_rect(&chrome, OverlayMode::Switcher).unwrap();
        assert_eq!(
            (sw, sh),
            (crate::SWITCHER_WIDTH as i32, crate::SWITCHER_HEIGHT as i32)
        );
        assert_eq!(sx, (1280 - crate::SWITCHER_WIDTH as i32) / 2);
        assert_eq!(sy, (800 - crate::SWITCHER_HEIGHT as i32) / 2);
        let (cx, cy, _, _) = overlay_rect(&chrome, OverlayMode::ClipboardHistory).unwrap();
        assert_eq!(cx, 1280 - crate::HISTORY_WIDTH as i32 - PANEL_MARGIN as i32);
        assert_eq!(cy, (TOPBAR_HEIGHT + PANEL_MARGIN) as i32);
        assert!(overlay_rect(&chrome, OverlayMode::None).is_none());
    }

    #[test]
    fn overlay_rows_stay_inside_panel_line_grid() {
        assert_eq!(overlay_row_at(ui::PANEL_LINE_START_Y, 5), Some(0));
        assert_eq!(
            overlay_row_at(ui::PANEL_LINE_START_Y + ui::PANEL_LINE_STEP * 4, 5),
            Some(4)
        );
        assert_eq!(overlay_row_at(0, CLIPBOARD_HISTORY_LINES), None);
    }
}

pub(crate) fn show_chrome(chrome: &Chrome) -> rt::Result<()> {
    rt::surface_set_visibility(chrome.desktop_handle, true)?;
    rt::surface_set_visibility(chrome.topbar_handle, true)?;
    rt::surface_set_visibility(chrome.launcher_handle, true)?;
    rt::surface_set_visibility(chrome.status_handle, true)?;
    rt::surface_set_visibility(chrome.switcher_handle, false)?;
    rt::surface_set_visibility(chrome.palette_handle, false)?;
    rt::surface_set_visibility(chrome.notifications_handle, false)?;
    rt::surface_set_visibility(chrome.clipboard_handle, false)?;
    rt::surface_set_visibility(chrome.media_handle, false)?;
    rt::surface_set_visibility(chrome.login_handle, false)?;
    rt::surface_set_visibility(chrome.cursor_handle, true)?;
    Ok(())
}
