use core::{ptr, slice};

use serviceos_userspace_runtime as rt;
use rt::DisplayPixelFormat;

use crate::types::{
    DEFAULT_BACKGROUND_RGB, DamageRect, MAX_FRAMEBUFFER_BYTES, SurfaceSlot, Surfaces,
    is_cursor_surface,
};

static mut FRAMEBUFFER_BYTES: [u8; MAX_FRAMEBUFFER_BYTES] = [0; MAX_FRAMEBUFFER_BYTES];
static mut BASE_FRAMEBUFFER_BYTES: [u8; MAX_FRAMEBUFFER_BYTES] = [0; MAX_FRAMEBUFFER_BYTES];

pub(crate) fn compose_and_present(
    output_handle: rt::Handle,
    output: rt::DisplayOutputInfo,
    surfaces: &Surfaces,
) -> rt::Result<()> {
    let byte_len = output.byte_len as usize;
    let base = base_framebuffer_slice(byte_len);
    compose_base_frame(base, output, surfaces);
    let frame = framebuffer_slice(byte_len);
    frame.copy_from_slice(base);
    overlay_cursor_surfaces(frame, output, surfaces);
    let _ = rt::display_output_present(output_handle, &frame[..byte_len])?;
    Ok(())
}

pub(crate) fn cursor_present(
    output_handle: rt::Handle,
    output: rt::DisplayOutputInfo,
    surfaces: &Surfaces,
    damage: DamageRect,
) -> rt::Result<()> {
    let byte_len = output.byte_len as usize;
    let frame = framebuffer_slice(byte_len);
    let base = base_framebuffer_slice(byte_len);
    restore_damage_from_base(frame, base, output, damage);
    overlay_cursor_surfaces(frame, output, surfaces);
    let _ = rt::display_output_present(output_handle, &frame[..byte_len])?;
    Ok(())
}

fn compose_base_frame(frame: &mut [u8], output: rt::DisplayOutputInfo, surfaces: &Surfaces) {
    fill_frame(frame, output, DEFAULT_BACKGROUND_RGB);
    let mut order = [0usize; crate::types::MAX_SURFACES];
    let mut count = 0usize;
    for (index, surface) in surfaces.iter().enumerate() {
        if surface.occupied && surface.visible && !is_cursor_surface(surface) {
            order[count] = index;
            count += 1;
        }
    }
    sort_surfaces_by_z(&mut order[..count], surfaces);
    for index in order[..count].iter().copied() {
        draw_surface(frame, output, &surfaces[index]);
    }
}

fn overlay_cursor_surfaces(frame: &mut [u8], output: rt::DisplayOutputInfo, surfaces: &Surfaces) {
    let mut order = [0usize; crate::types::MAX_SURFACES];
    let mut count = 0usize;
    for (index, surface) in surfaces.iter().enumerate() {
        if surface.occupied && surface.visible && is_cursor_surface(surface) {
            order[count] = index;
            count += 1;
        }
    }
    sort_surfaces_by_z(&mut order[..count], surfaces);
    for index in order[..count].iter().copied() {
        draw_surface(frame, output, &surfaces[index]);
    }
}

fn sort_surfaces_by_z(order: &mut [usize], surfaces: &Surfaces) {
    for idx in 1..order.len() {
        let key = order[idx];
        let key_z = surfaces[key].z_order;
        let mut cursor = idx;
        while cursor > 0 && surfaces[order[cursor - 1]].z_order > key_z {
            order[cursor] = order[cursor - 1];
            cursor -= 1;
        }
        order[cursor] = key;
    }
}

fn restore_damage_from_base(
    frame: &mut [u8],
    base: &[u8],
    output: rt::DisplayOutputInfo,
    damage: DamageRect,
) {
    let start_x = damage.x.max(0) as usize;
    let start_y = damage.y.max(0) as usize;
    let end_x = ((damage.x + damage.width as i32).max(0) as usize).min(output.width as usize);
    let end_y = ((damage.y + damage.height as i32).max(0) as usize).min(output.height as usize);
    if start_x >= end_x || start_y >= end_y {
        return;
    }

    let bytes_per_pixel = output.bytes_per_pixel as usize;
    for py in start_y..end_y {
        let row_start = (py * output.stride as usize + start_x) * bytes_per_pixel;
        let row_end = (py * output.stride as usize + end_x) * bytes_per_pixel;
        frame[row_start..row_end].copy_from_slice(&base[row_start..row_end]);
    }
}

fn fill_frame(frame: &mut [u8], output: rt::DisplayOutputInfo, rgb: u32) {
    for y in 0..output.height as usize {
        for x in 0..output.width as usize {
            write_pixel(frame, output, x, y, rgb);
        }
    }
}

fn draw_surface(frame: &mut [u8], output: rt::DisplayOutputInfo, surface: &SurfaceSlot) {
    draw_rect(
        frame,
        output,
        surface.x,
        surface.y,
        surface.width,
        surface.height,
        surface.fill_rgb,
    );
    if surface.buffer.attached() {
        draw_surface_buffer(frame, output, surface);
    }
    for rect in surface.rects.iter().filter(|rect| rect.occupied && rect.visible) {
        draw_rect(
            frame,
            output,
            surface.x.saturating_add(rect.x),
            surface.y.saturating_add(rect.y),
            rect.width,
            rect.height,
            rect.color_rgb,
        );
    }
    for label in surface.labels.iter().filter(|label| label.occupied) {
        draw_label(
            frame,
            output,
            surface.x.saturating_add(label.x),
            surface.y.saturating_add(label.y),
            label.color_rgb,
            &label.bytes[..label.len],
        );
    }
}

fn draw_surface_buffer(frame: &mut [u8], output: rt::DisplayOutputInfo, surface: &SurfaceSlot) {
    let buffer = surface.buffer;
    let width = surface.width.min(buffer.width);
    let height = surface.height.min(buffer.height);
    if width == 0 || height == 0 {
        return;
    }

    let start_x = surface.x.max(0) as usize;
    let start_y = surface.y.max(0) as usize;
    let end_x = ((surface.x + width as i32).max(0) as usize).min(output.width as usize);
    let end_y = ((surface.y + height as i32).max(0) as usize).min(output.height as usize);
    if start_x >= end_x || start_y >= end_y {
        return;
    }

    let clip_left = if surface.x < 0 { (-surface.x) as usize } else { 0 };
    let clip_top = if surface.y < 0 { (-surface.y) as usize } else { 0 };
    let visible_width = end_x - start_x;
    let total_bytes = buffer.height as usize * buffer.stride_pixels as usize * 4;
    if buffer.mapped_ptr.is_null() || total_bytes == 0 {
        return;
    }
    let bytes = unsafe { slice::from_raw_parts(buffer.mapped_ptr as *const u8, total_bytes) };

    for row_index in 0..(end_y - start_y) {
        let source_y = clip_top + row_index;
        let source_offset = ((source_y * buffer.stride_pixels as usize) + clip_left) * 4;
        for column in 0..visible_width {
            let base = source_offset + column * 4;
            if base + 3 >= bytes.len() {
                break;
            }
            let rgb = u32::from_le_bytes([
                bytes[base],
                bytes[base + 1],
                bytes[base + 2],
                bytes[base + 3],
            ]);
            write_pixel(frame, output, start_x + column, start_y + row_index, rgb & 0x00ff_ffff);
        }
    }
}

fn draw_rect(
    frame: &mut [u8],
    output: rt::DisplayOutputInfo,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    rgb: u32,
) {
    let start_x = x.max(0) as usize;
    let start_y = y.max(0) as usize;
    let end_x = ((x + width as i32).max(0) as usize).min(output.width as usize);
    let end_y = ((y + height as i32).max(0) as usize).min(output.height as usize);
    if start_x >= end_x || start_y >= end_y {
        return;
    }

    for py in start_y..end_y {
        for px in start_x..end_x {
            write_pixel(frame, output, px, py, rgb);
        }
    }
}

fn draw_label(
    frame: &mut [u8],
    output: rt::DisplayOutputInfo,
    x: i32,
    y: i32,
    color_rgb: u32,
    text: &[u8],
) {
    for (index, byte) in text.iter().copied().enumerate() {
        let ch = normalize_glyph(byte);
        let glyph_x = x.saturating_add((index * rt::BITMAP_GLYPH_ADVANCE) as i32);
        draw_glyph(frame, output, glyph_x, y, color_rgb, ch);
    }
}

fn draw_glyph(
    frame: &mut [u8],
    output: rt::DisplayOutputInfo,
    x: i32,
    y: i32,
    color_rgb: u32,
    ch: u8,
) {
    let rows = rt::bitmap_glyph_rows(ch);
    for (row_index, bits) in rows
        .iter()
        .copied()
        .enumerate()
        .take(rt::BITMAP_GLYPH_HEIGHT)
    {
        for column in 0..rt::BITMAP_GLYPH_WIDTH {
            if (bits >> (rt::BITMAP_GLYPH_WIDTH - 1 - column)) & 1 == 0 {
                continue;
            }
            let px = x.saturating_add(column as i32);
            let py = y.saturating_add(row_index as i32);
            if px < 0 || py < 0 {
                continue;
            }
            write_pixel(frame, output, px as usize, py as usize, color_rgb);
        }
    }
}

fn normalize_glyph(byte: u8) -> u8 {
    rt::normalize_bitmap_glyph(byte)
}

fn write_pixel(frame: &mut [u8], output: rt::DisplayOutputInfo, x: usize, y: usize, rgb: u32) {
    let offset = (y * output.stride as usize + x) * output.bytes_per_pixel as usize;
    if offset + 3 >= frame.len() {
        return;
    }

    let red = ((rgb >> 16) & 0xff) as u8;
    let green = ((rgb >> 8) & 0xff) as u8;
    let blue = (rgb & 0xff) as u8;
    match output.pixel_format {
        x if x == DisplayPixelFormat::Xrgb8888 as u32 => {
            frame[offset] = red;
            frame[offset + 1] = green;
            frame[offset + 2] = blue;
            frame[offset + 3] = 0;
        }
        _ => {
            frame[offset] = blue;
            frame[offset + 1] = green;
            frame[offset + 2] = red;
            frame[offset + 3] = 0;
        }
    }
}

fn framebuffer_slice(len: usize) -> &'static mut [u8] {
    unsafe {
        core::slice::from_raw_parts_mut(ptr::addr_of_mut!(FRAMEBUFFER_BYTES).cast::<u8>(), len)
    }
}

fn base_framebuffer_slice(len: usize) -> &'static mut [u8] {
    unsafe {
        core::slice::from_raw_parts_mut(
            ptr::addr_of_mut!(BASE_FRAMEBUFFER_BYTES).cast::<u8>(),
            len,
        )
    }
}
