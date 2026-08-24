use super::*;

pub(crate) const SHADOW_RECT_BASE: u32 = 7;
pub(crate) const SHADOW_STRIP_COUNT: usize = 8;
pub(crate) const SHADOW_OUTER_PX: u32 = 4;
pub(crate) const SHADOW_INNER_PX: u32 = 2;
pub(crate) const SHADOW_SOFT_RGB: u32 = 0x0d1626;
pub(crate) const SHADOW_STRONG_RGB: u32 = 0x070d18;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ShadowStrip {
    pub(crate) slot: u32,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) color_rgb: u32,
}

fn edge_strips(
    band_px: u32,
    width: u32,
    height: u32,
    color_rgb: u32,
    first_slot: u32,
) -> [ShadowStrip; 4] {
    let span = width.saturating_add(band_px * 2);
    [
        ShadowStrip {
            slot: first_slot,
            x: -(band_px as i32),
            y: -(band_px as i32),
            width: span,
            height: band_px,
            color_rgb,
        },
        ShadowStrip {
            slot: first_slot + 1,
            x: -(band_px as i32),
            y: height as i32,
            width: span,
            height: band_px,
            color_rgb,
        },
        ShadowStrip {
            slot: first_slot + 2,
            x: -(band_px as i32),
            y: 0,
            width: band_px,
            height,
            color_rgb,
        },
        ShadowStrip {
            slot: first_slot + 3,
            x: width as i32,
            y: 0,
            width: band_px,
            height,
            color_rgb,
        },
    ]
}

pub(crate) fn shadow_strips(width: u32, height: u32) -> [ShadowStrip; SHADOW_STRIP_COUNT] {
    let mut strips = [ShadowStrip {
        slot: 0,
        x: 0,
        y: 0,
        width: 0,
        height: 0,
        color_rgb: 0,
    }; SHADOW_STRIP_COUNT];
    strips[..4].copy_from_slice(&edge_strips(
        SHADOW_OUTER_PX,
        width,
        height,
        SHADOW_SOFT_RGB,
        SHADOW_RECT_BASE,
    ));
    strips[4..].copy_from_slice(&edge_strips(
        SHADOW_INNER_PX,
        width,
        height,
        SHADOW_STRONG_RGB,
        SHADOW_RECT_BASE + 4,
    ));
    strips
}

pub(crate) fn apply_window_shadow(
    surface_handle: rt::Handle,
    width: u32,
    height: u32,
) -> rt::Result<()> {
    for strip in shadow_strips(width, height) {
        rt::surface_set_rect(
            surface_handle,
            strip.slot,
            strip.x,
            strip.y,
            strip.width,
            strip.height,
            strip.color_rgb,
            true,
        )?;
    }
    Ok(())
}

pub(crate) fn clear_window_shadow(surface_handle: rt::Handle) -> rt::Result<()> {
    for slot in SHADOW_RECT_BASE..SHADOW_RECT_BASE + SHADOW_STRIP_COUNT as u32 {
        rt::surface_set_rect(surface_handle, slot, 0, 0, 0, 0, 0, false)?;
    }
    Ok(())
}

pub(crate) fn sync_focus_shadow(state: &mut DesktopState) {
    let target = state
        .focused_app
        .and_then(|app_id| app_slot_index(&state.apps, app_id))
        .filter(|&index| {
            let slot = &state.apps[index];
            slot.window.visible() && slot.workspace_id == state.active_workspace
        })
        .map(|index| {
            (
                state.apps[index].window.surface_id,
                state.apps[index].window.surface_handle,
                state.apps[index].window.width,
                state.apps[index].window.height,
            )
        });
    match target {
        Some((surface_id, _handle, width, height))
            if surface_id == state.shadow_surface_id
                && width == state.shadow_width
                && height == state.shadow_height => {}
        Some((surface_id, handle, width, height)) => {
            discard_shadow(state);
            match apply_window_shadow(handle, width, height) {
                Ok(()) => {
                    state.shadow_surface_id = surface_id;
                    state.shadow_surface_handle = handle;
                    state.shadow_width = width;
                    state.shadow_height = height;
                }
                Err(_) => {}
            }
        }
        None => discard_shadow(state),
    }
}

fn discard_shadow(state: &mut DesktopState) {
    if state.shadow_surface_id != 0 && state.shadow_surface_handle != rt::INVALID_HANDLE {
        let _ = clear_window_shadow(state.shadow_surface_handle);
    }
    state.shadow_surface_id = 0;
    state.shadow_surface_handle = rt::INVALID_HANDLE;
    state.shadow_width = 0;
    state.shadow_height = 0;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{WINDOW_MIN_HEIGHT, WINDOW_MIN_WIDTH};

    #[test]
    fn strips_never_cover_window_interior() {
        for (width, height) in [
            (WINDOW_MIN_WIDTH, WINDOW_MIN_HEIGHT),
            (420u32, 240u32),
            (1024u32, 768u32),
        ] {
            for strip in shadow_strips(width, height) {
                assert!(strip.width > 0 && strip.height > 0);
                let inside_x = strip.x >= 0 && (strip.x + strip.width as i32) <= width as i32;
                let inside_y = strip.y >= 0 && (strip.y + strip.height as i32) <= height as i32;
                assert!(
                    !(inside_x && inside_y),
                    "strip {:?} overlaps {}x{} interior",
                    strip,
                    width,
                    height
                );
            }
        }
    }

    #[test]
    fn strip_slots_are_sequential_and_bands_use_distinct_shades() {
        let strips = shadow_strips(500, 300);
        for (index, strip) in strips.iter().enumerate() {
            assert_eq!(strip.slot, SHADOW_RECT_BASE + index as u32);
        }
        for strip in &strips[..4] {
            assert_eq!(strip.color_rgb, SHADOW_SOFT_RGB);
        }
        for strip in &strips[4..] {
            assert_eq!(strip.color_rgb, SHADOW_STRONG_RGB);
        }
        assert!(SHADOW_STRONG_RGB < SHADOW_SOFT_RGB);
    }

    #[test]
    fn outer_band_wraps_wider_than_inner_band() {
        let (width, height) = (400u32, 260u32);
        let strips = shadow_strips(width, height);
        assert_eq!(strips[0].width, width + SHADOW_OUTER_PX * 2);
        assert_eq!(strips[4].width, width + SHADOW_INNER_PX * 2);
        assert_eq!(strips[0].y, -(SHADOW_OUTER_PX as i32));
        assert_eq!(strips[4].y, -(SHADOW_INNER_PX as i32));
        assert_eq!(strips[1].y, height as i32);
        assert_eq!(strips[2].height, height);
        assert_eq!(strips[3].x, width as i32);
    }

    #[test]
    fn strips_track_window_size_changes() {
        let small = shadow_strips(WINDOW_MIN_WIDTH, WINDOW_MIN_HEIGHT);
        let large = shadow_strips(900, 600);
        assert_ne!(small[0].width, large[0].width);
        assert_ne!(small[1].y, large[1].y);
    }
}
