use core::{ptr, slice};

use rt::DisplayPixelFormat;
use serviceos_userspace_runtime as rt;

use crate::types::{
    DEFAULT_BACKGROUND_RGB, DamageRect, MAX_FRAMEBUFFER_BYTES, SurfaceSlot, Surfaces,
    active_buffer, is_cursor_surface,
};

static mut FRAMEBUFFER_BYTES: [u8; MAX_FRAMEBUFFER_BYTES] = [0; MAX_FRAMEBUFFER_BYTES];
static mut BASE_FRAMEBUFFER_BYTES: [u8; MAX_FRAMEBUFFER_BYTES] = [0; MAX_FRAMEBUFFER_BYTES];
static mut PRESENTED_FRAMEBUFFER_BYTES: [u8; MAX_FRAMEBUFFER_BYTES] = [0; MAX_FRAMEBUFFER_BYTES];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PresentOutcome {
    pub(crate) skipped: bool,
    pub(crate) saved_bytes: u64,
    /// Bytes the kernel did not have to copy this present because the flush
    /// was narrowed to damaged scanline bands (distinct from noop skips,
    /// which issue no kernel present at all).
    pub(crate) band_saved_bytes: u64,
}

impl PresentOutcome {
    pub(crate) const fn presented() -> Self {
        Self {
            skipped: false,
            saved_bytes: 0,
            band_saved_bytes: 0,
        }
    }

    pub(crate) const fn noop(saved_bytes: u64) -> Self {
        Self {
            skipped: true,
            saved_bytes,
            band_saved_bytes: 0,
        }
    }

    pub(crate) const fn banded(saved_bytes: u64) -> Self {
        Self {
            skipped: false,
            saved_bytes: 0,
            band_saved_bytes: saved_bytes,
        }
    }
}

/// Upper bound on scanline bands flushed per present; more runs than this
/// falls back to presenting the whole clip region honestly.
pub(crate) const MAX_FLUSH_BANDS: usize = 8;

/// Strict-subset threshold: bands flush only while the changed area stays
/// under half of the frame (`changed * 2 < frame_total`).
const PARTIAL_FLUSH_DIVISOR: u64 = 2;

/// Half-open run of framebuffer scanlines `[start_y, end_y)` to flush.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ScanBand {
    pub(crate) start_y: u32,
    pub(crate) end_y: u32,
}

/// Merge half-open row spans into disjoint bands, uniting overlapping or
/// adjacent spans (damage rects commonly touch row edges). Returns the band
/// count, or `None` when the merged result would exceed the capacity.
pub(crate) fn merge_row_spans(
    spans: &[(u32, u32)],
    bands: &mut [ScanBand],
) -> Option<usize> {
    let mut count = 0usize;
    for &(start, end) in spans {
        if end <= start {
            continue;
        }
        let mut start = start;
        let mut end = end;
        let mut write = 0usize;
        let mut merged_existing = false;
        while write < count {
            let band = bands[write];
            if band.end_y < start {
                write += 1;
                continue;
            }
            if band.start_y > end {
                // Keep bands ordered by shifting the tail down one slot.
                if count >= bands.len() {
                    return None;
                }
                let mut shift = count;
                while shift > write {
                    bands[shift] = bands[shift - 1];
                    shift -= 1;
                }
                bands[write] = ScanBand { start_y: start, end_y: end };
                count += 1;
                merged_existing = true;
                break;
            }
            // Overlapping or adjacent: absorb into the existing band.
            start = start.min(band.start_y);
            end = end.max(band.end_y);
            bands[write] = ScanBand { start_y: start, end_y: end };
            merged_existing = true;
            // Absorb any later bands the grown span now touches.
            let mut read = write + 1;
            while read < count && bands[read].start_y <= end {
                end = end.max(bands[read].end_y);
                bands[write].end_y = end;
                read += 1;
            }
            while read < count {
                bands[write + 1] = bands[read];
                write += 1;
                read += 1;
            }
            count = write + 1;
            break;
        }
        if merged_existing {
            continue;
        }
        if write >= count {
            if count >= bands.len() {
                return None;
            }
            bands[count] = ScanBand { start_y: start, end_y: end };
            count += 1;
        }
    }
    Some(count)
}

/// What the presenter should do for this frame given the diff between the
/// composed frame and the presented shadow.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BandAction {
    /// Frame matches the shadow inside the clip: no kernel present at all.
    Skip(u64),
    /// Flush only these scanline bands; carries bytes not copied.
    Bands { saved_bytes: u64 },
    /// Fall back to presenting the whole clip region in one call.
    WholeClip,
}

#[derive(Clone, Copy)]
pub(crate) struct BandFlushPlan {
    pub(crate) action: BandAction,
    pub(crate) bands: [ScanBand; MAX_FLUSH_BANDS],
    pub(crate) band_count: usize,
}

impl BandFlushPlan {
    fn full() -> Self {
        Self {
            action: BandAction::WholeClip,
            bands: [ScanBand { start_y: 0, end_y: 0 }; MAX_FLUSH_BANDS],
            band_count: 0,
        }
    }
}

/// Compare `frame` against the presented shadow inside `clip` (`None` = the
/// whole visible frame) and decide how to flush. Rows are compared over the
/// clipped column range only, so stride padding never counts as damage.
pub(crate) fn plan_band_flush(
    frame: &[u8],
    presented: &[u8],
    output: rt::DisplayOutputInfo,
    clip: Option<DamageRect>,
    allow_partial: bool,
) -> BandFlushPlan {
    let Some((start_y, end_y, col_start, col_end)) = flush_span(output, clip) else {
        return BandFlushPlan::full();
    };
    if frame.len() != presented.len() {
        return BandFlushPlan::full();
    }
    let stride_bytes = output.stride as usize * output.bytes_per_pixel as usize;
    let frame_total_bytes = output.height as u64 * stride_bytes as u64;
    let cmp_bytes = col_end - col_start;

    let mut spans = [(0u32, 0u32); MAX_FLUSH_BANDS];
    let mut span_count = 0usize;
    let mut overflow = false;
    let mut run_start: Option<u32> = None;
    for row in start_y..end_y {
        let offset = row as usize * stride_bytes;
        // Compare only the clipped column range; anchoring at column 0
        // would read a prefix that can sit entirely outside the damage.
        let changed =
            frame[offset + col_start..offset + col_end] != presented[offset + col_start..offset + col_end];
        if changed && run_start.is_none() {
            run_start = Some(row);
        }
        if !changed {
            if let Some(start) = run_start.take() {
                if span_count >= spans.len() {
                    overflow = true;
                    break;
                }
                spans[span_count] = (start, row);
                span_count += 1;
            }
        }
    }
    if overflow {
        return BandFlushPlan::full();
    }
    if let Some(start) = run_start.take() {
        if span_count >= spans.len() {
            return BandFlushPlan::full();
        }
        spans[span_count] = (start, end_y);
        span_count += 1;
    }

    let mut plan = BandFlushPlan::full();
    let Some(band_count) = merge_row_spans(&spans[..span_count], &mut plan.bands) else {
        return BandFlushPlan::full();
    };
    plan.band_count = band_count;

    let span_rows = (end_y - start_y) as u64;
    let would_flush_bytes = span_rows * cmp_bytes as u64;
    if band_count == 0 {
        plan.action = BandAction::Skip(would_flush_bytes);
        return plan;
    }
    let changed_bytes: u64 = plan.bands[..band_count]
        .iter()
        .map(|band| (band.end_y - band.start_y) as u64 * cmp_bytes as u64)
        .sum();
    let subset_of_frame = changed_bytes.saturating_mul(PARTIAL_FLUSH_DIVISOR) < frame_total_bytes;
    if !allow_partial || !subset_of_frame || changed_bytes >= would_flush_bytes {
        return plan;
    }
    plan.action = BandAction::Bands {
        saved_bytes: would_flush_bytes - changed_bytes,
    };
    plan
}

/// Visible-row range plus the clipped column byte range for `clip`
/// (`None` = full visible width), mirroring the clamping rules of
/// `region_byte_span`.
fn flush_span(
    output: rt::DisplayOutputInfo,
    clip: Option<DamageRect>,
) -> Option<(u32, u32, usize, usize)> {
    let bpp = output.bytes_per_pixel as usize;
    if output.width == 0 || output.height == 0 || bpp == 0 {
        return None;
    }
    let (start_x, end_x, start_y, end_y) = match clip {
        None => (0usize, output.width as usize, 0u32, output.height),
        Some(rect) => {
            let sx = rect.x.max(0) as usize;
            let sy = rect.y.max(0) as u32;
            let ex = ((rect.x.saturating_add(rect.width as i32)).max(0) as usize)
                .min(output.width as usize);
            let ey = ((rect.y.saturating_add(rect.height as i32)).max(0) as u32)
                .min(output.height);
            (sx, ex, sy, ey)
        }
    };
    if start_x >= end_x || start_y >= end_y {
        return None;
    }
    Some((
        start_y,
        end_y,
        start_x * bpp,
        end_x * bpp,
    ))
}

pub(crate) fn compose_and_present(
    output_handle: rt::Handle,
    output: rt::DisplayOutputInfo,
    surfaces: &Surfaces,
    presented: &mut [u8],
    allow_noop_skip: bool,
) -> rt::Result<PresentOutcome> {
    let byte_len = output.byte_len as usize;
    let base = base_framebuffer_slice(byte_len);
    compose_base_frame(base, output, surfaces);
    let frame = framebuffer_slice(byte_len);
    frame.copy_from_slice(base);
    overlay_cursor_surfaces(frame, output, surfaces);
    present_full_inner(
        output_handle,
        output,
        &frame[..byte_len],
        presented,
        allow_noop_skip,
    )
}

pub(crate) fn cursor_present(
    output_handle: rt::Handle,
    output: rt::DisplayOutputInfo,
    surfaces: &Surfaces,
    damage: DamageRect,
    presented: &mut [u8],
    allow_noop_skip: bool,
) -> rt::Result<PresentOutcome> {
    let byte_len = output.byte_len as usize;
    let frame = framebuffer_slice(byte_len);
    let base = base_framebuffer_slice(byte_len);
    restore_damage_from_base(frame, base, output, damage);
    overlay_cursor_surfaces(frame, output, surfaces);
    present_damage_inner(
        output_handle,
        output,
        &frame[..byte_len],
        presented,
        damage,
        allow_noop_skip,
    )
}

pub(crate) fn compose_damage_and_present(
    output_handle: rt::Handle,
    output: rt::DisplayOutputInfo,
    surfaces: &Surfaces,
    damage: DamageRect,
    presented: &mut [u8],
    allow_noop_skip: bool,
) -> rt::Result<PresentOutcome> {
    let byte_len = output.byte_len as usize;
    let base = base_framebuffer_slice(byte_len);
    compose_base_damage(base, output, surfaces, damage);
    let frame = framebuffer_slice(byte_len);
    restore_damage_from_base(frame, base, output, damage);
    overlay_cursor_surfaces_damage(frame, output, surfaces, damage);
    present_damage_inner(
        output_handle,
        output,
        &frame[..byte_len],
        presented,
        damage,
        allow_noop_skip,
    )
}

fn region_byte_span(output: rt::DisplayOutputInfo, damage: DamageRect) -> Option<RegionSpan> {
    let start_x = damage.x.max(0) as usize;
    let start_y = damage.y.max(0) as usize;
    let end_x =
        ((damage.x.saturating_add(damage.width as i32)).max(0) as usize).min(output.width as usize);
    let end_y = ((damage.y.saturating_add(damage.height as i32)).max(0) as usize)
        .min(output.height as usize);
    if start_x >= end_x || start_y >= end_y {
        return None;
    }
    let stride_bytes = output.stride as usize * output.bytes_per_pixel as usize;
    Some(RegionSpan {
        row_start: start_y,
        row_end: end_y,
        col_start: start_x * output.bytes_per_pixel as usize,
        col_end: end_x * output.bytes_per_pixel as usize,
        stride_bytes,
    })
}

struct RegionSpan {
    row_start: usize,
    row_end: usize,
    col_start: usize,
    col_end: usize,
    stride_bytes: usize,
}

impl RegionSpan {
    fn update_rows(&self, presented: &mut [u8], frame: &[u8]) {
        for row in self.row_start..self.row_end {
            let offset = row * self.stride_bytes;
            let range = offset + self.col_start..offset + self.col_end;
            presented[range.clone()].copy_from_slice(&frame[range]);
        }
    }
}

fn present_damage_inner(
    output_handle: rt::Handle,
    output: rt::DisplayOutputInfo,
    frame: &[u8],
    presented: &mut [u8],
    damage: DamageRect,
    allow_noop_skip: bool,
) -> rt::Result<PresentOutcome> {
    if allow_noop_skip {
        let plan = plan_band_flush(frame, presented, output, Some(damage), true);
        match plan.action {
            BandAction::Skip(saved) => return Ok(PresentOutcome::noop(saved)),
            BandAction::Bands { saved_bytes } => {
                present_bands(output_handle, output, frame, presented, &plan)?;
                return Ok(PresentOutcome::banded(saved_bytes));
            }
            BandAction::WholeClip => {}
        }
    }
    rt::display_output_present_damage(
        output_handle,
        frame,
        damage.x,
        damage.y,
        damage.width,
        damage.height,
    )?;
    if presented.len() == frame.len() {
        if let Some(span) = region_byte_span(output, damage) {
            span.update_rows(presented, frame);
        }
    }
    Ok(PresentOutcome::presented())
}

fn present_full_inner(
    output_handle: rt::Handle,
    output: rt::DisplayOutputInfo,
    frame: &[u8],
    presented: &mut [u8],
    allow_noop_skip: bool,
) -> rt::Result<PresentOutcome> {
    if allow_noop_skip {
        let plan = plan_band_flush(frame, presented, output, None, true);
        match plan.action {
            BandAction::Skip(saved) => return Ok(PresentOutcome::noop(saved)),
            BandAction::Bands { saved_bytes } => {
                present_bands(output_handle, output, frame, presented, &plan)?;
                return Ok(PresentOutcome::banded(saved_bytes));
            }
            BandAction::WholeClip => {}
        }
    }
    rt::display_output_present(output_handle, frame)?;
    if presented.len() == frame.len() {
        presented.copy_from_slice(frame);
    }
    Ok(PresentOutcome::presented())
}

/// Flush the planned scanline bands through per-band kernel present-damage
/// calls (full-width rows) and mirror exactly those rows into the shadow.
fn present_bands(
    output_handle: rt::Handle,
    output: rt::DisplayOutputInfo,
    frame: &[u8],
    presented: &mut [u8],
    plan: &BandFlushPlan,
) -> rt::Result<()> {
    for band in plan.bands[..plan.band_count].iter().copied() {
        rt::display_output_present_damage(
            output_handle,
            frame,
            0,
            band.start_y as i32,
            output.width,
            band.end_y - band.start_y,
        )?;
    }
    if presented.len() == frame.len() {
        let stride_bytes = output.stride as usize * output.bytes_per_pixel as usize;
        let row_bytes = output.width as usize * output.bytes_per_pixel as usize;
        for band in plan.bands[..plan.band_count].iter().copied() {
            for row in band.start_y..band.end_y {
                let offset = row as usize * stride_bytes;
                let range = offset..offset + row_bytes;
                presented[range.clone()].copy_from_slice(&frame[range]);
            }
        }
    }
    Ok(())
}

pub(crate) fn presented_frame_slice(len: usize) -> &'static mut [u8] {
    unsafe {
        slice::from_raw_parts_mut(
            ptr::addr_of_mut!(PRESENTED_FRAMEBUFFER_BYTES).cast::<u8>(),
            len,
        )
    }
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

fn compose_base_damage(
    frame: &mut [u8],
    output: rt::DisplayOutputInfo,
    surfaces: &Surfaces,
    damage: DamageRect,
) {
    draw_rect_clipped(
        frame,
        output,
        0,
        0,
        output.width,
        output.height,
        DEFAULT_BACKGROUND_RGB,
        damage,
    );
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
        draw_surface_clipped(frame, output, &surfaces[index], Some(damage));
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

fn overlay_cursor_surfaces_damage(
    frame: &mut [u8],
    output: rt::DisplayOutputInfo,
    surfaces: &Surfaces,
    damage: DamageRect,
) {
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
        draw_surface_clipped(frame, output, &surfaces[index], Some(damage));
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
    draw_surface_clipped(frame, output, surface, None);
}

fn draw_surface_clipped(
    frame: &mut [u8],
    output: rt::DisplayOutputInfo,
    surface: &SurfaceSlot,
    clip: Option<DamageRect>,
) {
    draw_rect_impl(
        frame,
        output,
        surface.x,
        surface.y,
        surface.width,
        surface.height,
        surface.fill_rgb,
        clip,
    );
    if active_buffer(surface).is_some() {
        draw_surface_buffer(frame, output, surface, clip);
    }
    for rect in surface
        .rects
        .iter()
        .filter(|rect| rect.occupied && rect.visible)
    {
        draw_rect_impl(
            frame,
            output,
            surface.x.saturating_add(rect.x),
            surface.y.saturating_add(rect.y),
            rect.width,
            rect.height,
            rect.color_rgb,
            clip,
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
            clip,
        );
    }
}

fn draw_surface_buffer(
    frame: &mut [u8],
    output: rt::DisplayOutputInfo,
    surface: &SurfaceSlot,
    clip: Option<DamageRect>,
) {
    let Some(buffer) = active_buffer(surface) else {
        return;
    };
    let width = surface.width.min(buffer.width);
    let height = surface.height.min(buffer.height);
    if width == 0 || height == 0 {
        return;
    }

    let (start_x, start_y, end_x, end_y) =
        clip_rect(output, surface.x, surface.y, width, height, clip);
    if start_x >= end_x || start_y >= end_y {
        return;
    }

    let source_x = start_x.saturating_sub(surface.x.max(0) as usize)
        + if surface.x < 0 {
            (-surface.x) as usize
        } else {
            0
        };
    let source_y_base = start_y.saturating_sub(surface.y.max(0) as usize)
        + if surface.y < 0 {
            (-surface.y) as usize
        } else {
            0
        };
    let visible_width = end_x - start_x;
    let total_bytes = buffer.height as usize * buffer.stride_pixels as usize * 4;
    if buffer.mapped_ptr.is_null() || total_bytes == 0 {
        return;
    }
    let bytes = unsafe { slice::from_raw_parts(buffer.mapped_ptr as *const u8, total_bytes) };

    for row_index in 0..(end_y - start_y) {
        let source_y = source_y_base + row_index;
        let source_offset = ((source_y * buffer.stride_pixels as usize) + source_x) * 4;
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
            write_pixel(
                frame,
                output,
                start_x + column,
                start_y + row_index,
                rgb & 0x00ff_ffff,
            );
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
    draw_rect_impl(frame, output, x, y, width, height, rgb, None);
}

fn draw_rect_clipped(
    frame: &mut [u8],
    output: rt::DisplayOutputInfo,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    rgb: u32,
    clip: DamageRect,
) {
    draw_rect_impl(frame, output, x, y, width, height, rgb, Some(clip));
}

fn draw_rect_impl(
    frame: &mut [u8],
    output: rt::DisplayOutputInfo,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    rgb: u32,
    clip: Option<DamageRect>,
) {
    let (start_x, start_y, end_x, end_y) = clip_rect(output, x, y, width, height, clip);
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
    clip: Option<DamageRect>,
) {
    for (index, byte) in text.iter().copied().enumerate() {
        let ch = normalize_glyph(byte);
        let glyph_x = x.saturating_add((index * rt::BITMAP_GLYPH_ADVANCE) as i32);
        draw_glyph(frame, output, glyph_x, y, color_rgb, ch, clip);
    }
}

fn draw_glyph(
    frame: &mut [u8],
    output: rt::DisplayOutputInfo,
    x: i32,
    y: i32,
    color_rgb: u32,
    ch: u8,
    clip: Option<DamageRect>,
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
            if let Some(clip) = clip {
                if px < clip.x
                    || py < clip.y
                    || px >= clip.x.saturating_add(clip.width as i32)
                    || py >= clip.y.saturating_add(clip.height as i32)
                {
                    continue;
                }
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
        core::slice::from_raw_parts_mut(ptr::addr_of_mut!(BASE_FRAMEBUFFER_BYTES).cast::<u8>(), len)
    }
}

fn clip_rect(
    output: rt::DisplayOutputInfo,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    clip: Option<DamageRect>,
) -> (usize, usize, usize, usize) {
    let mut start_x = x.max(0) as usize;
    let mut start_y = y.max(0) as usize;
    let mut end_x = ((x + width as i32).max(0) as usize).min(output.width as usize);
    let mut end_y = ((y + height as i32).max(0) as usize).min(output.height as usize);
    if let Some(clip) = clip {
        start_x = start_x.max(clip.x.max(0) as usize);
        start_y = start_y.max(clip.y.max(0) as usize);
        end_x =
            end_x.min(((clip.x + clip.width as i32).max(0) as usize).min(output.width as usize));
        end_y =
            end_y.min(((clip.y + clip.height as i32).max(0) as usize).min(output.height as usize));
    }
    (start_x, start_y, end_x, end_y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(width: u32, height: u32, stride: u32, bpp: u32) -> rt::DisplayOutputInfo {
        rt::DisplayOutputInfo {
            backend: 0,
            state: 0,
            pixel_format: 0,
            reserved: 0,
            width,
            height,
            stride,
            bytes_per_pixel: bpp,
            byte_len: (stride as u64) * (height as u64) * (bpp as u64),
            present_count: 0,
        }
    }

    fn rect(x: i32, y: i32, width: u32, height: u32) -> DamageRect {
        DamageRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn region_span_clamps_to_output_bounds() {
        let out = output(8, 8, 8, 4);
        let span = region_byte_span(out, rect(0, 0, 100, 100)).unwrap();
        assert_eq!(span.col_start, 0);
        assert_eq!(span.col_end, 8 * 4);
        assert_eq!(span.row_start, 0);
        assert_eq!(span.row_end, 8);
    }

    #[test]
    fn region_span_rejects_empty_and_offscreen() {
        let out = output(8, 8, 8, 4);
        assert!(region_byte_span(out, rect(0, 0, 0, 4)).is_none());
        assert!(region_byte_span(out, rect(-20, -20, 4, 4)).is_none());
        assert!(region_byte_span(out, rect(100, 0, 4, 4)).is_none());
    }

    #[test]
    fn row_spans_merge_overlapping_and_adjacent_rects() {
        let mut bands = [ScanBand { start_y: 0, end_y: 0 }; MAX_FLUSH_BANDS];
        // Two overlapping rects plus one touching edge and one disjoint.
        let count = merge_row_spans(
            &[(2, 5), (4, 9), (9, 12), (20, 22)],
            &mut bands,
        )
        .unwrap();
        assert_eq!(count, 2);
        assert_eq!(bands[0], ScanBand { start_y: 2, end_y: 12 });
        assert_eq!(bands[1], ScanBand { start_y: 20, end_y: 22 });
    }

    #[test]
    fn row_spans_ignore_empty_and_report_capacity_overflow() {
        let mut bands = [ScanBand { start_y: 0, end_y: 0 }; MAX_FLUSH_BANDS];
        assert_eq!(merge_row_spans(&[(3, 3), (1, 1)], &mut bands), Some(0));
        let wide: Vec<(u32, u32)> =
            (0..MAX_FLUSH_BANDS as u32 + 2).map(|y| (y * 10, y * 10 + 5)).collect();
        assert!(merge_row_spans(&wide, &mut bands).is_none());
    }

    #[test]
    fn band_plan_skips_identical_region_with_span_savings() {
        let out = output(4, 4, 4, 4);
        let frame = vec![7u8; out.byte_len as usize];
        let presented = frame.clone();
        // Savings count only visible compared bytes, never stride padding.
        let plan = plan_band_flush(&frame, &presented, out, Some(rect(0, 1, 4, 2)), true);
        assert_eq!(plan.action, BandAction::Skip(4 * 4 * 2));
        // Whole-frame clip reports the full visible byte span.
        let whole = plan_band_flush(&frame, &presented, out, None, true);
        assert_eq!(whole.action, BandAction::Skip(out.byte_len));
    }

    #[test]
    fn band_plan_compares_at_damage_column_offset() {
        let out = output(64, 4, 64, 4);
        let mut frame = vec![0u8; out.byte_len as usize];
        let presented = vec![0u8; out.byte_len as usize];
        // Only the damaged columns change (x=48..56, rows 1..3). The row
        // prefix spanning [0, damage width) stays identical, so a compare
        // anchored at column 0 must not report the region as unchanged.
        for row in 1..3 {
            for x in 48..56 {
                let offset = (row as usize * 64 + x) * 4;
                frame[offset] = 0xff;
            }
        }
        let plan = plan_band_flush(&frame, &presented, out, Some(rect(48, 1, 8, 2)), true);
        assert!(!matches!(plan.action, BandAction::Skip(_)));
    }

    #[test]
    fn band_plan_flushes_bands_under_half_frame_and_tracks_saved_bytes() {
        let out = output(4, 8, 4, 4);
        let mut frame = vec![0u8; out.byte_len as usize];
        let presented = vec![0u8; out.byte_len as usize];
        // Change rows 0 and 7 only: two single-row bands, a quarter of the
        // frame — a strict subset under the <50% rule.
        for x in 0..4usize {
            let at = x * 4;
            frame[at] = 0xAA;
            let at = 7 * 16 + x * 4;
            frame[at] = 0xBB;
        }
        let plan = plan_band_flush(&frame, &presented, out, None, true);
        let saved = match plan.action {
            BandAction::Bands { saved_bytes } => saved_bytes,
            other => panic!("expected bands, got {other:?}"),
        };
        assert_eq!(plan.band_count, 2);
        assert_eq!(
            plan.bands[..2],
            [
                ScanBand { start_y: 0, end_y: 1 },
                ScanBand { start_y: 7, end_y: 8 }
            ]
        );
        // Would-flush is all 8 rows (128 bytes); only 32 bytes changed.
        assert_eq!(saved, 96);
    }

    #[test]
    fn band_plan_falls_back_to_whole_clip_at_half_frame() {
        let out = output(4, 4, 4, 4);
        let mut frame = vec![0u8; out.byte_len as usize];
        let presented = vec![1u8; out.byte_len as usize];
        // Rows 0-1 differ: exactly half the visible bytes -> not a strict
        // subset under the <50% rule.
        for offset in 0..2 * 16usize {
            frame[offset] = 9;
        }
        let plan = plan_band_flush(&frame, &presented, out, None, true);
        assert_eq!(plan.action, BandAction::WholeClip);
    }

    #[test]
    fn band_plan_ignores_stride_padding_damage() {
        let out = output(4, 2, 6, 4); // stride 6 > width 4: 8 padding bytes/row
        let mut frame = vec![0u8; out.byte_len as usize];
        let presented = vec![0u8; out.byte_len as usize];
        // Touch only stride padding; compared columns stay identical.
        for row in 0..2usize {
            frame[row * 24 + 4 * 4] = 0xEE;
            frame[row * 24 + 5 * 4] = 0xEE;
        }
        // Would-flush covers the two visible 16-byte rows only.
        let plan = plan_band_flush(&frame, &presented, out, None, true);
        assert_eq!(plan.action, BandAction::Skip(32));
    }

    #[test]
    fn band_plan_partial_disabled_or_short_shadow_uses_whole_clip() {
        let out = output(4, 4, 4, 4);
        let mut frame = vec![0u8; out.byte_len as usize];
        let presented = vec![0u8; out.byte_len as usize];
        frame[0] = 1; // single changed pixel
        let gated = plan_band_flush(&frame, &presented, out, None, false);
        assert_eq!(gated.action, BandAction::WholeClip);
        let short = plan_band_flush(&frame, &[], out, None, true);
        assert_eq!(short.action, BandAction::WholeClip);
    }

    #[test]
    fn outcome_records_band_savings_separate_from_noop_skips() {
        let mut stats = crate::types::PresentStats::default();
        stats.record(&PresentOutcome::banded(96));
        stats.record(&PresentOutcome::noop(64));
        stats.record(&PresentOutcome::banded(32));
        assert_eq!(stats.presents, 3);
        assert_eq!(stats.noop_skips, 1);
        assert_eq!(stats.noop_saved_bytes, 64);
        assert_eq!(stats.band_presents, 2);
        assert_eq!(stats.band_saved_bytes, 128);
    }
}
