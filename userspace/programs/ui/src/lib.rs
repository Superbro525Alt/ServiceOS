#![no_std]

use serviceos_userspace_runtime as rt;

pub const BG_DESKTOP: u32 = 0x132033;
pub const BG_PANEL: u32 = 0x1b283d;
pub const BG_WINDOW: u32 = 0x202a3d;
pub const BG_WINDOW_ALT: u32 = 0x24334a;
pub const ACCENT: u32 = 0x7cc6ff;
pub const ACCENT_DIM: u32 = 0x436b8a;
pub const TEXT_PRIMARY: u32 = 0xe7f1ff;
pub const TEXT_SECONDARY: u32 = 0xa6b9cf;
pub const TEXT_MUTED: u32 = 0x6e8198;
pub const STATUS_OK: u32 = 0x8de19d;
pub const STATUS_WARN: u32 = 0xf2c36b;
pub const TITLEBAR_HEIGHT: u32 = 28;
pub const WINDOW_BUTTON_SIZE: u32 = 12;
pub const WINDOW_BUTTON_TOP: i32 = 8;
pub const WINDOW_BUTTON_RIGHT_MARGIN: i32 = 10;
pub const WINDOW_BUTTON_GAP: i32 = 8;
pub const WINDOW_BORDER_THICKNESS: i32 = 6;
pub const CURSOR_OUTLINE: u32 = 0x0b1220;
pub const CURSOR_FILL: u32 = 0xf3f8ff;
pub const PANEL_LINE_START_Y: i32 = 42;
pub const PANEL_LINE_STEP: i32 = 14;

pub fn render_window(
    surface: rt::Handle,
    width: u32,
    height: u32,
    background_rgb: u32,
    accent_rgb: u32,
    title: &str,
    lines: &[&str],
) -> rt::Result<()> {
    render_window_state(
        surface,
        width,
        height,
        background_rgb,
        accent_rgb,
        title,
        lines,
        true,
    )
}

pub fn render_window_state(
    surface: rt::Handle,
    width: u32,
    height: u32,
    background_rgb: u32,
    accent_rgb: u32,
    title: &str,
    lines: &[&str],
    focused: bool,
) -> rt::Result<()> {
    let titlebar_rgb = if focused { accent_rgb } else { ACCENT_DIM };
    rt::surface_set_fill(surface, background_rgb)?;
    rt::surface_clear_scene(surface)?;
    rt::surface_set_rect(surface, 0, 0, 0, width, TITLEBAR_HEIGHT, titlebar_rgb, true)?;
    rt::surface_set_rect(
        surface,
        1,
        0,
        TITLEBAR_HEIGHT as i32,
        width,
        height.saturating_sub(TITLEBAR_HEIGHT),
        background_rgb,
        true,
    )?;
    rt::surface_set_label(surface, 0, 10, 9, TEXT_PRIMARY, title)?;
    let close_x = width as i32 - WINDOW_BUTTON_RIGHT_MARGIN - WINDOW_BUTTON_SIZE as i32;
    let minimize_x = close_x - WINDOW_BUTTON_GAP - WINDOW_BUTTON_SIZE as i32;
    let maximize_x = minimize_x - WINDOW_BUTTON_GAP - WINDOW_BUTTON_SIZE as i32;
    rt::surface_set_rect(
        surface,
        5,
        maximize_x,
        WINDOW_BUTTON_TOP,
        WINDOW_BUTTON_SIZE,
        WINDOW_BUTTON_SIZE,
        ACCENT,
        true,
    )?;
    rt::surface_set_rect(
        surface,
        2,
        minimize_x,
        WINDOW_BUTTON_TOP,
        WINDOW_BUTTON_SIZE,
        WINDOW_BUTTON_SIZE,
        TEXT_MUTED,
        true,
    )?;
    rt::surface_set_rect(
        surface,
        3,
        close_x,
        WINDOW_BUTTON_TOP,
        WINDOW_BUTTON_SIZE,
        WINDOW_BUTTON_SIZE,
        STATUS_WARN,
        true,
    )?;
    rt::surface_set_rect(surface, 6, maximize_x + 3, WINDOW_BUTTON_TOP + 3, 6, 6, BG_PANEL, true)?;
    rt::surface_set_label(surface, 14, minimize_x + 3, WINDOW_BUTTON_TOP + 2, BG_PANEL, "_")?;
    rt::surface_set_label(surface, 15, close_x + 3, WINDOW_BUTTON_TOP + 2, BG_PANEL, "X")?;
    for (index, line) in lines.iter().copied().enumerate() {
        rt::surface_set_label(
            surface,
            (index + 1 + 4) as u32,
            12,
            PANEL_LINE_START_Y + (index as i32 * PANEL_LINE_STEP),
            if index == 0 { TEXT_PRIMARY } else { TEXT_SECONDARY },
            line,
        )?;
    }
    Ok(())
}

pub fn render_panel(
    surface: rt::Handle,
    width: u32,
    height: u32,
    title: &str,
    lines: &[&str],
) -> rt::Result<()> {
    render_window_state(surface, width, height, BG_PANEL, ACCENT_DIM, title, lines, true)
}

pub fn render_status_panel(
    surface: rt::Handle,
    width: u32,
    height: u32,
    title: &str,
    lines: &[(&str, u32)],
) -> rt::Result<()> {
    rt::surface_set_fill(surface, BG_PANEL)?;
    rt::surface_clear_scene(surface)?;
    rt::surface_set_rect(surface, 0, 0, 0, width, 24, ACCENT_DIM, true)?;
    rt::surface_set_label(surface, 0, 8, 8, TEXT_PRIMARY, title)?;
    for (index, (line, color)) in lines.iter().copied().enumerate() {
        rt::surface_set_label(
            surface,
            (index + 1) as u32,
            10,
            34 + (index as i32 * PANEL_LINE_STEP),
            color,
            line,
        )?;
    }
    let _ = height;
    Ok(())
}

pub fn render_cursor(surface: rt::Handle, size: u32) -> rt::Result<()> {
    rt::surface_set_fill(surface, 0)?;
    rt::surface_clear_scene(surface)?;
    rt::surface_set_rect(surface, 0, 0, 0, 2, size, CURSOR_OUTLINE, true)?;
    rt::surface_set_rect(surface, 1, 0, 0, size, 2, CURSOR_OUTLINE, true)?;
    rt::surface_set_rect(surface, 2, 1, 1, 1, size.saturating_sub(2), CURSOR_FILL, true)?;
    rt::surface_set_rect(surface, 3, 1, 1, size.saturating_sub(2), 1, CURSOR_FILL, true)?;
    rt::surface_set_rect(surface, 4, 3, 3, 2, 6, CURSOR_OUTLINE, true)?;
    rt::surface_set_rect(surface, 5, 4, 4, 1, 5, CURSOR_FILL, true)?;
    Ok(())
}
