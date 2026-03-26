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

pub fn render_window(
    surface: rt::Handle,
    width: u32,
    height: u32,
    background_rgb: u32,
    accent_rgb: u32,
    title: &str,
    lines: &[&str],
) -> rt::Result<()> {
    rt::surface_set_fill(surface, background_rgb)?;
    rt::surface_clear_scene(surface)?;
    rt::surface_set_rect(surface, 0, 0, 0, width, 28, accent_rgb, true)?;
    rt::surface_set_rect(
        surface,
        1,
        0,
        28,
        width,
        height.saturating_sub(28),
        background_rgb,
        true,
    )?;
    rt::surface_set_label(surface, 0, 10, 9, TEXT_PRIMARY, title)?;
    for (index, line) in lines.iter().copied().enumerate() {
        rt::surface_set_label(
            surface,
            (index + 1) as u32,
            12,
            42 + (index as i32 * 14),
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
    render_window(surface, width, height, BG_PANEL, ACCENT_DIM, title, lines)
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
        rt::surface_set_label(surface, (index + 1) as u32, 10, 34 + (index as i32 * 14), color, line)?;
    }
    let _ = height;
    Ok(())
}
