use super::*;

/// Drag-gesture geometry: edge-snap zones, rubber-band preview strips, and
/// workspace-overview tile layout. Pure math lives here so host unit tests
/// cover thresholds without a graphics service.
pub(crate) const SNAP_EDGE_BAND_PX: i32 = 16;
pub(crate) const SNAP_BAR_PX: u32 = 6;
pub(crate) const SNAP_PREVIEW_RGB: u32 = 0x3d7fd6;
pub(crate) const SNAP_PREVIEW_ALT_RGB: u32 = 0xd67f3d;
pub(crate) const GESTURE_RECT_SLOTS: u32 = 4;
pub(crate) const OVERVIEW_TILE_WIDTH: u32 = 100;
pub(crate) const OVERVIEW_TILE_HEIGHT: u32 = 84;
pub(crate) const OVERVIEW_TILE_PAD: i32 = 12;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum SnapZone {
    #[default]
    None,
    LeftHalf,
    RightHalf,
    MinimizeBottom,
}

/// Which snap zone (if any) the pointer is in during a window drag. The
/// bottom band wins over side bands so bottom corners minimize instead of
/// tile; the topbar strip never snaps.
pub(crate) fn snap_zone_at(x: i32, y: i32, output_width: u32, output_height: u32) -> SnapZone {
    let width = output_width as i32;
    let height = output_height as i32;
    if x < 0 || y < 0 || x >= width || y >= height {
        return SnapZone::None;
    }
    if y >= height - SNAP_EDGE_BAND_PX {
        return SnapZone::MinimizeBottom;
    }
    if y < TOPBAR_HEIGHT as i32 {
        return SnapZone::None;
    }
    if x < SNAP_EDGE_BAND_PX {
        return SnapZone::LeftHalf;
    }
    if x >= width - SNAP_EDGE_BAND_PX {
        return SnapZone::RightHalf;
    }
    SnapZone::None
}

/// Full-screen rect a half-snapped window would occupy (below the topbar).
/// Odd widths give the right half the extra pixel.
pub(crate) fn snap_target_rect(
    zone: SnapZone,
    output_width: u32,
    output_height: u32,
) -> Option<(i32, i32, u32, u32)> {
    let area_height = output_height.saturating_sub(TOPBAR_HEIGHT);
    match zone {
        SnapZone::LeftHalf => Some((0, TOPBAR_HEIGHT as i32, output_width / 2, area_height)),
        SnapZone::RightHalf => {
            let left_span = output_width / 2;
            Some((
                left_span as i32,
                TOPBAR_HEIGHT as i32,
                output_width - left_span,
                area_height,
            ))
        }
        SnapZone::MinimizeBottom | SnapZone::None => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PreviewStrip {
    pub(crate) slot: u32,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) color_rgb: u32,
}

const EMPTY_STRIP: PreviewStrip = PreviewStrip {
    slot: 0,
    x: 0,
    y: 0,
    width: 0,
    height: 0,
    color_rgb: 0,
};

/// Rubber-band preview: an outline of the target half for side snaps or a
/// full-width bar along the bottom edge for drop-to-minimize. At most four
/// strips (one per gesture rect slot).
pub(crate) fn zone_preview_strips(
    zone: SnapZone,
    output_width: u32,
    output_height: u32,
) -> ([PreviewStrip; GESTURE_RECT_SLOTS as usize], usize) {
    let mut strips = [EMPTY_STRIP; GESTURE_RECT_SLOTS as usize];
    let bar = SNAP_BAR_PX;
    let mut count = 0usize;
    let mut push = |slot: u32, x: i32, y: i32, w: u32, h: u32, color: u32| {
        if count < strips.len() {
            strips[count] = PreviewStrip {
                slot,
                x,
                y,
                width: w,
                height: h,
                color_rgb: color,
            };
            count += 1;
        }
    };
    match zone {
        SnapZone::LeftHalf | SnapZone::RightHalf => {
            let Some((x, y, w, h)) = snap_target_rect(zone, output_width, output_height) else {
                return (strips, 0);
            };
            let color = if zone == SnapZone::LeftHalf {
                SNAP_PREVIEW_RGB
            } else {
                SNAP_PREVIEW_ALT_RGB
            };
            push(0, x, y, w, bar, color);
            push(1, x, (y + h as i32) - bar as i32, w, bar, color);
            push(2, x, y, bar, h, color);
            push(3, (x + w as i32) - bar as i32, y, bar, h, color);
        }
        SnapZone::MinimizeBottom => {
            push(
                0,
                0,
                output_height.saturating_sub(bar) as i32,
                output_width,
                bar,
                SNAP_PREVIEW_RGB,
            );
        }
        SnapZone::None => {}
    }
    (strips, count)
}

/// Workspace-overview tile rect in overlay-local coordinates (2x2 grid).
pub(crate) fn overview_tile_rect(index: usize) -> (i32, i32, u32, u32) {
    let column = index % 2;
    let row = index / 2;
    let x = OVERVIEW_TILE_PAD + column as i32 * (OVERVIEW_TILE_WIDTH as i32 + OVERVIEW_TILE_PAD);
    let y = ui::TITLEBAR_HEIGHT as i32
        + OVERVIEW_TILE_PAD
        + row as i32 * (OVERVIEW_TILE_HEIGHT as i32 + OVERVIEW_TILE_PAD);
    (x, y, OVERVIEW_TILE_WIDTH, OVERVIEW_TILE_HEIGHT)
}

/// Tile under an overlay-local pointer position, if any.
pub(crate) fn overview_tile_at(local_x: i32, local_y: i32) -> Option<usize> {
    for index in 0..WORKSPACE_COUNT as usize {
        let (tx, ty, tw, th) = overview_tile_rect(index);
        if local_x >= tx && local_y >= ty && local_x < tx + tw as i32 && local_y < ty + th as i32 {
            return Some(index);
        }
    }
    None
}

/// Selection stepping across workspace ids 1..=WORKSPACE_COUNT with clamping
/// at both ends (no wrap: mission-control grids clamp).
pub(crate) fn step_workspace_selection(current: u32, delta: i32) -> u32 {
    let next = current as i64 + delta as i64;
    next.clamp(1, WORKSPACE_COUNT as i64) as u32
}

pub(crate) fn update_snap_preview(
    state: &mut crate::DesktopState,
    zone: SnapZone,
) -> rt::Result<()> {
    let surface = state.chrome.gesture_handle;
    let show = zone != SnapZone::None;
    let (strips, count) =
        zone_preview_strips(zone, state.chrome.output_width, state.chrome.output_height);
    for strip in strips.iter() {
        if strip.slot < count as u32 {
            rt::surface_set_rect(
                surface,
                strip.slot,
                strip.x,
                strip.y,
                strip.width,
                strip.height,
                strip.color_rgb,
                true,
            )?;
        } else {
            rt::surface_set_rect(surface, strip.slot, 0, 0, 0, 0, 0, false)?;
        }
    }
    rt::surface_set_visibility(surface, show)
}

pub(crate) fn hide_snap_preview(state: &mut crate::DesktopState) -> rt::Result<()> {
    update_snap_preview(state, SnapZone::None)
}

/// Content-drag ghost chip: a bounded label ("[/]"/"[]" kind icon + file
/// name, palette doc-row style) on the gesture overlay surface that follows
/// the cursor while a drag is armed. Slot 4 keeps the snap-preview rect
/// slots 0..3 untouched; label slot 0 is unused on this surface otherwise.
pub(crate) const GHOST_RECT_SLOT: u32 = 4;
pub(crate) const GHOST_LABEL_SLOT: u32 = 0;
pub(crate) const GHOST_CHIP_HEIGHT: u32 = 15;
pub(crate) const GHOST_CHIP_PAD_X: i32 = 5;
pub(crate) const GHOST_CHIP_PAD_Y: i32 = 4;
pub(crate) const GHOST_TEXT_MAX: usize = 56;
pub(crate) const GHOST_ADVANCE: i32 = 6;
const GHOST_ICON_DIR: &[u8] = b"[/]";
const GHOST_ICON_FILE: &[u8] = b"[]";

/// Geometry + text of the ghost chip for a drag over `pointer`. The chip
/// hangs below-right of the cursor, clamped inside the output so the chip
/// is always fully visible; integer geometry only (1:1 blit, no
/// resampling), consistent with the magnifier precedent.
pub(crate) struct GhostChip {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) text: [u8; GHOST_TEXT_MAX],
    pub(crate) text_len: usize,
}

/// File name within a drag path: the segment after the last `/`, with a
/// trailing `/` (directory drag marker) stripped first.
pub(crate) fn ghost_file_name(path: &[u8]) -> &[u8] {
    let trimmed = if let Some((&b'/', head)) = path.split_last() {
        head
    } else {
        path
    };
    let start = trimmed
        .iter()
        .rposition(|byte| *byte == b'/')
        .map_or(0, |pos| pos + 1);
    &trimmed[start..]
}

/// Chip label: kind icon, space, file name, and a " +N" suffix naming the
/// additional files when a drag carries more than one. Never exceeds
/// `GHOST_TEXT_MAX` bytes (the graphics-service label budget).
pub(crate) fn ghost_chip_text(path: &[u8], count: usize) -> ([u8; GHOST_TEXT_MAX], usize) {
    let mut text = [0u8; GHOST_TEXT_MAX];
    let icon = if path.ends_with(b"/") {
        GHOST_ICON_DIR
    } else {
        GHOST_ICON_FILE
    };
    let mut len = 0usize;
    let push = |bytes: &[u8], text: &mut [u8], len: &mut usize| {
        for byte in bytes {
            if *len < text.len() {
                text[*len] = *byte;
                *len += 1;
            }
        }
    };
    push(icon, &mut text, &mut len);
    push(b" ", &mut text, &mut len);
    let name = ghost_file_name(path);
    let room_for_name = text.len().saturating_sub(len + 6);
    push(&name[..room_for_name.min(name.len())], &mut text, &mut len);
    if count > 1 {
        push(b" +", &mut text, &mut len);
        let extra = (count - 1).min(9) as u8;
        push(&[b'0' + extra], &mut text, &mut len);
    }
    (text, len)
}

pub(crate) fn ghost_chip(
    path: &[u8],
    count: usize,
    pointer_x: i32,
    pointer_y: i32,
    output_width: u32,
    output_height: u32,
) -> GhostChip {
    let (text, text_len) = ghost_chip_text(path, count);
    let width = (text_len as i32 * GHOST_ADVANCE + GHOST_CHIP_PAD_X * 2 + 1).max(1) as u32;
    let height = GHOST_CHIP_HEIGHT;
    let raw_x = pointer_x + 10;
    let raw_y = pointer_y + 12;
    let x = raw_x.clamp(0, (output_width.saturating_sub(width)) as i32);
    let y = raw_y.clamp(0, (output_height.saturating_sub(height)) as i32);
    GhostChip {
        x,
        y,
        width,
        height,
        text,
        text_len,
    }
}

/// Renders/moves the ghost chip under the pointer while a content drag is
/// armed; no-op when no drag is live.
pub(crate) fn update_drag_ghost(state: &mut crate::DesktopState, x: i32, y: i32) -> rt::Result<()> {
    let Some(drag) = state.content_drag.as_ref() else {
        return Ok(());
    };
    let chip = ghost_chip(
        &drag.path[..drag.path_len],
        drag.count,
        x,
        y,
        state.chrome.output_width,
        state.chrome.output_height,
    );
    let Ok(text) = core::str::from_utf8(&chip.text[..chip.text_len]) else {
        return Ok(());
    };
    let surface = state.chrome.gesture_handle;
    rt::surface_set_rect(
        surface,
        GHOST_RECT_SLOT,
        chip.x,
        chip.y,
        chip.width,
        chip.height,
        ui::BG_PANEL,
        true,
    )?;
    rt::surface_set_label(
        surface,
        GHOST_LABEL_SLOT,
        chip.x + GHOST_CHIP_PAD_X,
        chip.y + GHOST_CHIP_PAD_Y,
        ui::TEXT_PRIMARY,
        text,
    )?;
    rt::surface_set_visibility(surface, true)
}

/// Clears the ghost chip (drop, cancel, expiry, or source-app exit).
pub(crate) fn hide_drag_ghost(state: &mut crate::DesktopState) -> rt::Result<()> {
    let surface = state.chrome.gesture_handle;
    rt::surface_set_rect(surface, GHOST_RECT_SLOT, 0, 0, 0, 0, 0, false)?;
    rt::surface_set_label(surface, GHOST_LABEL_SLOT, 0, 0, 0, "")?;
    rt::surface_set_visibility(surface, false)
}

#[cfg(test)]
mod tests {
    use super::super::drag::{CONTENT_DRAG_MAX_FILES, CONTENT_PAYLOAD_MAX};
    use super::*;

    const W: u32 = 1280;
    const H: u32 = 800;

    #[test]
    fn edge_bands_trigger_side_snaps_below_topbar() {
        assert_eq!(snap_zone_at(0, 300, W, H), SnapZone::LeftHalf);
        assert_eq!(
            snap_zone_at(SNAP_EDGE_BAND_PX - 1, 300, W, H),
            SnapZone::LeftHalf
        );
        assert_eq!(snap_zone_at(SNAP_EDGE_BAND_PX, 300, W, H), SnapZone::None);
        assert_eq!(snap_zone_at(W as i32 - 1, 300, W, H), SnapZone::RightHalf);
        assert_eq!(
            snap_zone_at(W as i32 - SNAP_EDGE_BAND_PX, 300, W, H),
            SnapZone::RightHalf
        );
        assert_eq!(
            snap_zone_at(W as i32 - SNAP_EDGE_BAND_PX - 1, 300, W, H),
            SnapZone::None
        );
    }

    #[test]
    fn bottom_band_wins_and_topbar_never_snaps() {
        assert_eq!(
            snap_zone_at(4, H as i32 - 1, W, H),
            SnapZone::MinimizeBottom
        );
        assert_eq!(
            snap_zone_at(W as i32 - 4, H as i32 - SNAP_EDGE_BAND_PX, W, H),
            SnapZone::MinimizeBottom
        );
        assert_eq!(
            snap_zone_at(0, H as i32 - SNAP_EDGE_BAND_PX - 1, W, H),
            SnapZone::LeftHalf
        );
        assert_eq!(snap_zone_at(0, 0, W, H), SnapZone::None);
        assert_eq!(
            snap_zone_at(0, (TOPBAR_HEIGHT - 1) as i32, W, H),
            SnapZone::None
        );
        assert_eq!(snap_zone_at(640, 400, W, H), SnapZone::None);
    }

    #[test]
    fn offscreen_points_reject_all_zones() {
        assert_eq!(snap_zone_at(-1, 400, W, H), SnapZone::None);
        assert_eq!(snap_zone_at(400, -1, W, H), SnapZone::None);
        assert_eq!(snap_zone_at(W as i32, 400, W, H), SnapZone::None);
        assert_eq!(snap_zone_at(400, H as i32, W, H), SnapZone::None);
        assert_eq!(snap_zone_at(-100, -100, W, H), SnapZone::None);
    }

    #[test]
    fn half_targets_split_below_topbar_with_odd_remainder_right() {
        let (lx, ly, lw, lh) = snap_target_rect(SnapZone::LeftHalf, W, H).unwrap();
        assert_eq!((lx, ly), (0, TOPBAR_HEIGHT as i32));
        assert_eq!((lw, lh), (W / 2, H - TOPBAR_HEIGHT));
        let (rx, ry, rw, rh) = snap_target_rect(SnapZone::RightHalf, 1281, H).unwrap();
        assert_eq!((rx, ry), ((1281 / 2) as i32, TOPBAR_HEIGHT as i32));
        assert_eq!((rw, rh), (1281 - 1281 / 2, H - TOPBAR_HEIGHT));
        assert!(snap_target_rect(SnapZone::MinimizeBottom, W, H).is_none());
        assert!(snap_target_rect(SnapZone::None, W, H).is_none());
    }

    #[test]
    fn halves_tile_to_full_area_without_overlap_or_gap() {
        for width in [640u32, 1024, 1280, 1281] {
            let (_, _, lw, _) = snap_target_rect(SnapZone::LeftHalf, width, H).unwrap();
            let (rx, _, rw, _) = snap_target_rect(SnapZone::RightHalf, width, H).unwrap();
            assert_eq!(lw + rw, width);
            assert_eq!(rx, lw as i32);
        }
    }

    #[test]
    fn preview_strips_outline_half_and_bar_bottom() {
        let (strips, count) = zone_preview_strips(SnapZone::LeftHalf, W, H);
        assert_eq!(count, 4);
        for (index, strip) in strips.iter().enumerate() {
            assert_eq!(strip.slot, index as u32);
            assert!(strip.width > 0 && strip.height > 0);
            assert_eq!(strip.color_rgb, SNAP_PREVIEW_RGB);
        }
        // Outline stays inside the target rect.
        let (x, y, w, h) = snap_target_rect(SnapZone::LeftHalf, W, H).unwrap();
        assert_eq!(strips[0].y, y);
        assert_eq!(strips[1].y + strips[1].height as i32, y + h as i32);
        assert_eq!(strips[2].x, x);
        assert_eq!(strips[3].x + strips[3].width as i32, x + w as i32);

        let (min_strips, min_count) = zone_preview_strips(SnapZone::MinimizeBottom, W, H);
        assert_eq!(min_count, 1);
        assert_eq!(min_strips[0].width, W);
        assert_eq!(min_strips[0].y + min_strips[0].height as i32, H as i32);

        let (none_strips, none_count) = zone_preview_strips(SnapZone::None, W, H);
        assert_eq!(none_count, 0);
        assert!(none_strips.iter().all(|strip| strip.width == 0));
    }

    #[test]
    fn overview_tiles_form_clamped_grid() {
        for index in 0..WORKSPACE_COUNT as usize {
            let (x, y, w, h) = overview_tile_rect(index);
            assert_eq!((w, h), (OVERVIEW_TILE_WIDTH, OVERVIEW_TILE_HEIGHT));
            assert!(x >= 0 && y >= 0);
        }
        let (x0, y0, _, _) = overview_tile_rect(0);
        let (x1, _, _, _) = overview_tile_rect(1);
        let (_, y2, _, _) = overview_tile_rect(2);
        assert!(x1 > x0);
        assert!(y2 > y0);
        assert_eq!(overview_tile_at(x0, y0), Some(0));
        assert_eq!(overview_tile_at(0, 0), None);
        assert_eq!(overview_tile_at(10_000, 10_000), None);
    }

    #[test]
    fn overview_selection_clamps_without_wrap() {
        assert_eq!(step_workspace_selection(1, -1), 1);
        assert_eq!(step_workspace_selection(2, -1), 1);
        assert_eq!(step_workspace_selection(3, 1), 4);
        assert_eq!(step_workspace_selection(4, 1), 4);
        assert_eq!(step_workspace_selection(2, 0), 2);
        assert_eq!(step_workspace_selection(1, -100), 1);
        assert_eq!(step_workspace_selection(4, 100), WORKSPACE_COUNT);
    }

    #[test]
    fn ghost_names_take_last_segment_and_strip_dir_slash() {
        assert_eq!(ghost_file_name(b"home/notes.txt"), b"notes.txt");
        assert_eq!(ghost_file_name(b"home/docs/"), b"docs");
        assert_eq!(ghost_file_name(b"lonely.txt"), b"lonely.txt");
        assert_eq!(ghost_file_name(b"/"), b"");
    }

    #[test]
    fn ghost_chip_text_shows_kind_icon_and_extra_count() {
        let (text, len) = ghost_chip_text(b"home/notes.txt", 1);
        assert_eq!(&text[..len], b"[] notes.txt");
        let (text, len) = ghost_chip_text(b"home/docs/", 3);
        assert_eq!(&text[..len], b"[/] docs +2");
    }

    #[test]
    fn ghost_chip_text_never_exceeds_label_budget() {
        let long_path = [b'a'; CONTENT_PAYLOAD_MAX];
        let (text, len) = ghost_chip_text(&long_path, CONTENT_DRAG_MAX_FILES);
        assert!(len <= GHOST_TEXT_MAX);
        assert!(text[..len].starts_with(b"[] "));
        assert!(text[len - 2..len] == *b"+3");
    }

    #[test]
    fn ghost_chip_hangs_below_right_and_clamps_inside_output() {
        let chip = ghost_chip(b"home/notes.txt", 1, 100, 100, W, H);
        assert_eq!((chip.x, chip.y), (110, 112));
        assert_eq!(chip.width, "[] notes.txt".len() as u32 * 6 + 11);
        let corner = ghost_chip(b"home/n.txt", 1, W as i32 - 2, H as i32 - 2, W, H);
        assert!(corner.x + corner.width as i32 <= W as i32);
        assert!(corner.y + corner.height as i32 <= H as i32);
        assert!(corner.x >= 0 && corner.y >= 0);
    }
}
