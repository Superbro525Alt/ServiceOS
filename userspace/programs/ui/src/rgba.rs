use serviceos_userspace_runtime as rt;

use crate::{
    ACCENT, ACCENT_DIM, BG_PANEL, STATUS_WARN, TEXT_MUTED, TEXT_PRIMARY, TITLEBAR_HEIGHT,
    WINDOW_BUTTON_GAP, WINDOW_BUTTON_RIGHT_MARGIN, WINDOW_BUTTON_SIZE, WINDOW_BUTTON_TOP,
};

pub fn fill_rgba8888_rect(
    frame: &mut [u8],
    stride_pixels: usize,
    frame_width: usize,
    frame_height: usize,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    color_rgb: u32,
) {
    let end_x = (x + width).min(frame_width);
    let end_y = (y + height).min(frame_height);
    for py in y..end_y {
        for px in x..end_x {
            rt::set_pixel_rgba8888(frame, stride_pixels, px, py, color_rgb);
        }
    }
}

pub fn draw_window_frame_rgba8888(
    frame: &mut [u8],
    stride_pixels: usize,
    frame_width: usize,
    frame_height: usize,
    focused: bool,
    body_rgb: u32,
    title: &str,
) {
    fill_rgba8888_rect(
        frame,
        stride_pixels,
        frame_width,
        frame_height,
        0,
        0,
        frame_width,
        frame_height,
        body_rgb,
    );
    fill_rgba8888_rect(
        frame,
        stride_pixels,
        frame_width,
        frame_height,
        0,
        0,
        frame_width,
        TITLEBAR_HEIGHT as usize,
        if focused { ACCENT } else { ACCENT_DIM },
    );
    draw_window_titlebar_rgba8888(frame, stride_pixels, frame_width, title);
}

pub fn draw_window_titlebar_rgba8888(
    frame: &mut [u8],
    stride_pixels: usize,
    frame_width: usize,
    title: &str,
) {
    let close_x = frame_width as i32 - WINDOW_BUTTON_RIGHT_MARGIN - WINDOW_BUTTON_SIZE as i32;
    let minimize_x = close_x - WINDOW_BUTTON_GAP - WINDOW_BUTTON_SIZE as i32;
    let maximize_x = minimize_x - WINDOW_BUTTON_GAP - WINDOW_BUTTON_SIZE as i32;
    fill_rgba8888_rect(
        frame,
        stride_pixels,
        frame_width,
        TITLEBAR_HEIGHT as usize,
        maximize_x.max(0) as usize,
        WINDOW_BUTTON_TOP.max(0) as usize,
        WINDOW_BUTTON_SIZE as usize,
        WINDOW_BUTTON_SIZE as usize,
        ACCENT,
    );
    fill_rgba8888_rect(
        frame,
        stride_pixels,
        frame_width,
        TITLEBAR_HEIGHT as usize,
        minimize_x.max(0) as usize,
        WINDOW_BUTTON_TOP.max(0) as usize,
        WINDOW_BUTTON_SIZE as usize,
        WINDOW_BUTTON_SIZE as usize,
        TEXT_MUTED,
    );
    fill_rgba8888_rect(
        frame,
        stride_pixels,
        frame_width,
        TITLEBAR_HEIGHT as usize,
        close_x.max(0) as usize,
        WINDOW_BUTTON_TOP.max(0) as usize,
        WINDOW_BUTTON_SIZE as usize,
        WINDOW_BUTTON_SIZE as usize,
        STATUS_WARN,
    );
    fill_rgba8888_rect(
        frame,
        stride_pixels,
        frame_width,
        TITLEBAR_HEIGHT as usize,
        (maximize_x + 3).max(0) as usize,
        (WINDOW_BUTTON_TOP + 3).max(0) as usize,
        6,
        6,
        BG_PANEL,
    );
    rt::draw_text_rgba8888(
        frame,
        stride_pixels,
        minimize_x + 3,
        WINDOW_BUTTON_TOP + 2,
        BG_PANEL,
        "_",
    );
    rt::draw_text_rgba8888(
        frame,
        stride_pixels,
        close_x + 3,
        WINDOW_BUTTON_TOP + 2,
        BG_PANEL,
        "X",
    );
    rt::draw_text_rgba8888(frame, stride_pixels, 10, 9, TEXT_PRIMARY, title);
}
