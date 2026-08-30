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

#[cfg(test)]
mod tests {
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
}
