use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;

use crate::state::{
    Chrome, CURSOR_SIZE, CURSOR_Z_ORDER, HISTORY_HEIGHT, HISTORY_WIDTH, LAUNCHER_WIDTH,
    PALETTE_HEIGHT, PALETTE_WIDTH, PANEL_MARGIN, SESSION_ID, STATUS_PANEL_HEIGHT,
    STATUS_PANEL_WIDTH, SWITCHER_HEIGHT, SWITCHER_WIDTH, TOPBAR_HEIGHT,
};

pub(crate) fn create_chrome(
    graphics_handle: rt::Handle,
    output_width: u32,
    output_height: u32,
) -> rt::Result<Chrome> {
    let (_, desktop_handle) = rt::graphics_surface_create(
        graphics_handle,
        SESSION_ID,
        0,
        0,
        output_width,
        output_height,
        0,
        ui::BG_DESKTOP,
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
        ui::BG_PANEL,
        false,
    )?;
    let (_, launcher_handle) = rt::graphics_surface_create(
        graphics_handle,
        SESSION_ID,
        PANEL_MARGIN as i32,
        (TOPBAR_HEIGHT + PANEL_MARGIN) as i32,
        LAUNCHER_WIDTH,
        264,
        2,
        ui::BG_PANEL,
        false,
    )?;
    let (_, status_handle) = rt::graphics_surface_create(
        graphics_handle,
        SESSION_ID,
        (output_width.saturating_sub(STATUS_PANEL_WIDTH + PANEL_MARGIN)) as i32,
        (TOPBAR_HEIGHT + PANEL_MARGIN) as i32,
        STATUS_PANEL_WIDTH,
        STATUS_PANEL_HEIGHT,
        2,
        ui::BG_PANEL,
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
        ui::BG_PANEL,
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
        ui::BG_PANEL,
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
        ui::BG_PANEL,
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
        ui::BG_PANEL,
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
        cursor_handle,
        output_width,
        output_height,
    })
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
    rt::surface_set_visibility(chrome.cursor_handle, true)?;
    Ok(())
}
