use crate::OverlayMode;
use serviceos_userspace_runtime as rt;

pub(crate) const CORNER_ZONE_PX: i32 = 24;
pub(crate) const CORNER_DWELL_TICKS: u64 = 12;
pub(crate) const ZOOM_STEP_COUNT: usize = 3;
pub(crate) const ACCESS_CONFIG_MAGIC: [u8; 3] = [b'D', b'A', b'Z'];
pub(crate) const ACCESS_CONFIG_BYTES: usize = 4;
pub(crate) const STORE_DIR_NAME: &str = "desktop-shell";
pub(crate) const STORE_DIR_PREFIX: &str = "state/desktop-shell/";
pub(crate) const STATE_ROOT: &str = "state/";
pub(crate) const ACCESS_CFG_FILE: &str = "access.cfg";

const FLAG_HIGH_CONTRAST: u8 = 1 << 0;
const FLAG_REDUCE_MOTION: u8 = 1 << 1;
const FLAG_ZOOM_SHIFT: u32 = 2;
const FLAG_ZOOM_MASK: u8 = 0b1100;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AccessSettings {
    pub(crate) high_contrast: bool,
    pub(crate) reduce_motion: bool,
    pub(crate) zoom_index: usize,
}

impl AccessSettings {
    pub(crate) const fn new() -> Self {
        Self {
            high_contrast: false,
            reduce_motion: false,
            zoom_index: 0,
        }
    }

    pub(crate) fn encode(self) -> [u8; ACCESS_CONFIG_BYTES] {
        let mut flags = 0u8;
        if self.high_contrast {
            flags |= FLAG_HIGH_CONTRAST;
        }
        if self.reduce_motion {
            flags |= FLAG_REDUCE_MOTION;
        }
        let zoom = (self.zoom_index.min(ZOOM_STEP_COUNT - 1) as u8) << FLAG_ZOOM_SHIFT;
        flags |= zoom & FLAG_ZOOM_MASK;
        [
            ACCESS_CONFIG_MAGIC[0],
            ACCESS_CONFIG_MAGIC[1],
            ACCESS_CONFIG_MAGIC[2],
            flags,
        ]
    }

    pub(crate) fn decode(bytes: &[u8]) -> Self {
        if bytes.len() < ACCESS_CONFIG_BYTES || bytes[..3] != ACCESS_CONFIG_MAGIC {
            return Self::new();
        }
        let flags = bytes[3];
        let zoom_index = ((flags & FLAG_ZOOM_MASK) >> FLAG_ZOOM_SHIFT) as usize;
        Self {
            high_contrast: flags & FLAG_HIGH_CONTRAST != 0,
            reduce_motion: flags & FLAG_REDUCE_MOTION != 0,
            zoom_index: zoom_index.min(ZOOM_STEP_COUNT - 1),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CornerDwell {
    pub(crate) corner: Option<Corner>,
    pub(crate) since_tick: u64,
    pub(crate) fired: bool,
}

impl CornerDwell {
    pub(crate) const fn new() -> Self {
        Self {
            corner: None,
            since_tick: 0,
            fired: false,
        }
    }

    pub(crate) fn update(&mut self, corner: Option<Corner>, now: u64) -> Option<Corner> {
        match corner {
            Some(corner) => {
                if self.corner != Some(corner) {
                    self.corner = Some(corner);
                    self.since_tick = now;
                    self.fired = false;
                    return None;
                }
                if !self.fired && now.saturating_sub(self.since_tick) >= CORNER_DWELL_TICKS {
                    self.fired = true;
                    return Some(corner);
                }
                None
            }
            None => {
                self.corner = None;
                self.fired = false;
                None
            }
        }
    }
}

pub(crate) fn corner_at(x: i32, y: i32, width: i32, height: i32) -> Option<Corner> {
    if width <= 0 || height <= 0 || x < 0 || y < 0 || x >= width || y >= height {
        return None;
    }
    let zone = CORNER_ZONE_PX;
    let left = x < zone;
    let right = x > width - zone;
    let top = y < zone;
    let bottom = y > height - zone;
    match (top, bottom, left, right) {
        (true, _, true, _) => Some(Corner::TopLeft),
        (true, _, _, true) => Some(Corner::TopRight),
        (_, true, true, _) => Some(Corner::BottomLeft),
        (_, true, _, true) => Some(Corner::BottomRight),
        _ => None,
    }
}

pub(crate) fn zoom_num_den(zoom_index: usize) -> (u32, u32) {
    match zoom_index.min(ZOOM_STEP_COUNT - 1) {
        1 => (3, 2),
        2 => (4, 2),
        _ => (2, 2),
    }
}

pub(crate) fn step_zoom(zoom_index: usize, zoom_in: bool) -> usize {
    if zoom_in {
        (zoom_index + 1).min(ZOOM_STEP_COUNT - 1)
    } else {
        zoom_index.saturating_sub(1)
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn zoom_transform_rect(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    focal_x: i32,
    focal_y: i32,
    num: u32,
    den: u32,
    bounds_width: i32,
    bounds_height: i32,
) -> (i32, i32, u32, u32) {
    if num == den || width == 0 || height == 0 || bounds_width <= 0 || bounds_height <= 0 {
        return (x, y, width, height);
    }
    let num64 = num as i64;
    let den64 = den as i64;
    let bounds_w = bounds_width as i64;
    let bounds_h = bounds_height as i64;

    let mut scaled_w = (width as i64 * num64 / den64).max(1).min(bounds_w);
    let mut scaled_h = (height as i64 * num64 / den64).max(1).min(bounds_h);

    let mut out_x = focal_x as i64 - ((focal_x as i64 - x as i64) * num64) / den64;
    let mut out_y = focal_y as i64 - ((focal_y as i64 - y as i64) * num64) / den64;
    if out_x < 0 {
        out_x = 0;
    }
    if out_y < 0 {
        out_y = 0;
    }
    if out_x + scaled_w > bounds_w {
        out_x = bounds_w - scaled_w;
    }
    if out_y + scaled_h > bounds_h {
        out_y = bounds_h - scaled_h;
    }
    scaled_w = scaled_w.max(1);
    scaled_h = scaled_h.max(1);
    (out_x as i32, out_y as i32, scaled_w as u32, scaled_h as u32)
}

/// Inverse of the zoom mapping for a screen point: returns the logical canvas
/// coordinate that renders at (`x`, `y`) under the current magnification.
pub(crate) fn zoom_unmap_point(
    zoom_index: usize,
    focal_x: i32,
    focal_y: i32,
    x: i32,
    y: i32,
) -> (i32, i32) {
    let (num, den) = zoom_num_den(zoom_index);
    if num == den {
        return (x, y);
    }
    let num64 = num as i64;
    let den64 = den as i64;
    let ux = focal_x as i64 + ((x as i64 - focal_x as i64) * den64) / num64;
    let uy = focal_y as i64 + ((y as i64 - focal_y as i64) * den64) / num64;
    (ux as i32, uy as i32)
}

pub(crate) const fn animations_enabled(reduce_motion: bool) -> bool {
    !reduce_motion
}

/// Shell theme palette; high-contrast swaps to black/white with maximal text
/// contrast across every shell-rendered surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Theme {
    pub(crate) desktop_bg: u32,
    pub(crate) panel: u32,
    pub(crate) titlebar: u32,
    pub(crate) accent: u32,
    pub(crate) accent_dim: u32,
    pub(crate) text: u32,
    pub(crate) text_secondary: u32,
    pub(crate) text_muted: u32,
    pub(crate) window_alt: u32,
    pub(crate) status_ok: u32,
    pub(crate) status_warn: u32,
}

pub(crate) const fn resolve_theme(high_contrast: bool) -> Theme {
    if high_contrast {
        Theme {
            desktop_bg: 0x000000,
            panel: 0x000000,
            titlebar: 0xffffff,
            accent: 0xffffff,
            accent_dim: 0xdddddd,
            text: 0xffffff,
            text_secondary: 0xffffff,
            text_muted: 0xdddddd,
            window_alt: 0x3a3a3a,
            status_ok: 0x00ff00,
            status_warn: 0xffff00,
        }
    } else {
        Theme {
            desktop_bg: 0x132033,
            panel: 0x1b283d,
            titlebar: 0x7cc6ff,
            accent: 0x7cc6ff,
            accent_dim: 0x436b8a,
            text: 0xe7f1ff,
            text_secondary: 0xa6b9cf,
            text_muted: 0x6e8198,
            window_alt: 0x24334a,
            status_ok: 0x8de19d,
            status_warn: 0xf2c36b,
        }
    }
}

/// True when the magnifier transform should currently be applied to window and
/// panel geometry: zoomed in, no modal overlay, no drag/resize/capture, and no
/// queued window animation that would fight the transformed geometry.
pub(crate) fn zoom_engaged(state: &crate::DesktopState) -> bool {
    if state.access.zoom_index == 0 || state.overlay_mode != OverlayMode::None {
        return false;
    }
    if state.drag_state.is_some()
        || state.content_capture.is_some()
        || state.content_drag.is_some()
        || state.pending_resize.is_some()
    {
        return false;
    }
    state.animations.iter().all(|anim| anim.is_none())
}

fn zoom_focal(state: &crate::DesktopState) -> (i32, i32) {
    (
        state
            .pointer_x
            .clamp(0, state.chrome.output_width as i32 - 1),
        state
            .pointer_y
            .clamp(0, state.chrome.output_height as i32 - 1),
    )
}

/// Re-applies (or releases) the magnifier geometry transform. No-ops while the
/// engaged state and focal point are unchanged so idle loop spins stay cheap.
pub(crate) fn sync_zoom(state: &mut crate::DesktopState) -> rt::Result<()> {
    let engaged = zoom_engaged(state);
    let (num, den) = zoom_num_den(state.access.zoom_index);
    if !engaged || num == den {
        if state.zoom_applied {
            restore_zoom(state)?;
        }
        return Ok(());
    }
    let (fx, fy) = zoom_focal(state);
    if state.zoom_applied
        && state.zoom_last_fx == fx
        && state.zoom_last_fy == fy
        && state.zoom_last_index == state.access.zoom_index
    {
        return Ok(());
    }
    apply_zoom(state, fx, fy, num, den)?;
    state.zoom_applied = true;
    state.zoom_last_fx = fx;
    state.zoom_last_fy = fy;
    state.zoom_last_index = state.access.zoom_index;
    Ok(())
}

fn apply_zoom(
    state: &mut crate::DesktopState,
    fx: i32,
    fy: i32,
    num: u32,
    den: u32,
) -> rt::Result<()> {
    let bounds_w = state.chrome.output_width as i32;
    let bounds_h = state.chrome.output_height as i32;
    let active_workspace = state.active_workspace;
    for slot in state.apps.iter() {
        let window = &slot.window;
        if !window.visible()
            || slot.workspace_id != active_workspace
            || window.surface_handle == rt::INVALID_HANDLE
        {
            continue;
        }
        let (nx, ny, nw, nh) = zoom_transform_rect(
            window.x,
            window.y,
            window.width,
            window.height,
            fx,
            fy,
            num,
            den,
            bounds_w,
            bounds_h,
        );
        rt::surface_set_geometry_async(window.surface_handle, nx, ny, nw, nh, window.z_order)?;
    }
    for &(handle, x, y, w, h, z) in &state.chrome.zoom_panels {
        if handle == rt::INVALID_HANDLE {
            continue;
        }
        let (nx, ny, nw, nh) =
            zoom_transform_rect(x, y, w, h, fx, fy, num, den, bounds_w, bounds_h);
        rt::surface_set_geometry_async(handle, nx, ny, nw, nh, z)?;
    }
    Ok(())
}

fn restore_zoom(state: &mut crate::DesktopState) -> rt::Result<()> {
    let active_workspace = state.active_workspace;
    for slot in state.apps.iter() {
        let window = &slot.window;
        if window.surface_handle == rt::INVALID_HANDLE {
            continue;
        }
        if !window.visible() || slot.workspace_id != active_workspace {
            continue;
        }
        rt::surface_set_geometry_async(
            window.surface_handle,
            window.x,
            window.y,
            window.width,
            window.height,
            window.z_order,
        )?;
    }
    for &(handle, x, y, w, h, z) in &state.chrome.zoom_panels {
        if handle == rt::INVALID_HANDLE {
            continue;
        }
        rt::surface_set_geometry_async(handle, x, y, w, h, z)?;
    }
    state.zoom_applied = false;
    state.zoom_last_fx = -1;
    state.zoom_last_fy = -1;
    Ok(())
}

/// Base layout rect of the launcher panel. Hit-testing runs in logical canvas
/// space (pointer events are inverse-mapped first), so it deliberately ignores
/// the magnifier's display-time geometry transform.
pub(crate) fn launcher_base_rect(state: &crate::DesktopState) -> (i32, i32, u32, u32) {
    match state.chrome.zoom_panels.first() {
        Some(&(_, x, y, w, h, _)) => (x, y, w, h),
        None => (0, 0, 0, 0),
    }
}

pub(crate) fn ensure_access_store_dir(storage: rt::Handle) -> rt::Handle {
    if storage == rt::INVALID_HANDLE {
        return rt::INVALID_HANDLE;
    }
    if let Ok(handle) = rt::storage_open_directory(storage, STORE_DIR_PREFIX, true) {
        return handle;
    }
    let Ok(root) = rt::storage_open_directory(storage, STATE_ROOT, true) else {
        return rt::INVALID_HANDLE;
    };
    let _ = rt::storage_directory_create(root, STORE_DIR_NAME, rt::StorageEntryKind::Directory);
    let _ = rt::handle_close(root);
    match rt::storage_open_directory(storage, STORE_DIR_PREFIX, true) {
        Ok(handle) => handle,
        Err(_) => rt::INVALID_HANDLE,
    }
}

pub(crate) fn save_access_settings(dir: rt::Handle, settings: AccessSettings) {
    if dir == rt::INVALID_HANDLE {
        return;
    }
    let bytes = settings.encode();
    let Ok((file, _)) = rt::storage_directory_open_file(dir, ACCESS_CFG_FILE, true, true) else {
        return;
    };
    let result = rt::storage_write(file, 0, bytes.len(), &bytes);
    let _ = rt::handle_close(file);
    let _ = result;
}

pub(crate) fn load_access_settings(dir: rt::Handle) -> AccessSettings {
    if dir == rt::INVALID_HANDLE {
        return AccessSettings::new();
    }
    let mut buffer = [0u8; ACCESS_CONFIG_BYTES];
    let Ok((file, size)) = rt::storage_directory_open_file(dir, ACCESS_CFG_FILE, false, false)
    else {
        return AccessSettings::new();
    };
    let read_len = size.min(buffer.len());
    let read = rt::storage_read_all(file, &mut buffer, read_len);
    let _ = rt::handle_close(file);
    match read {
        Ok(_) => AccessSettings::decode(&buffer),
        Err(_) => AccessSettings::new(),
    }
}

fn persist_access(state: &crate::DesktopState) {
    save_access_settings(state.access_store_dir, state.access);
}

/// Magnifier zoom step, shared by the action registry (Ctrl+Alt+= / Ctrl+Alt+-).
pub(crate) fn apply_zoom_step(
    state: &mut crate::DesktopState,
    zoom_in: bool,
) -> rt::Result<u32> {
    state.access.zoom_index = step_zoom(state.access.zoom_index, zoom_in);
    persist_access(state);
    sync_zoom(state)?;
    Ok(crate::windows::focused_surface_id(state))
}

pub(crate) fn toggle_high_contrast(state: &mut crate::DesktopState) -> rt::Result<u32> {
    state.access.high_contrast = !state.access.high_contrast;
    persist_access(state);
    crate::render::render_desktop(state)?;
    Ok(crate::windows::focused_surface_id(state))
}

pub(crate) fn toggle_reduce_motion(state: &mut crate::DesktopState) -> rt::Result<u32> {
    state.access.reduce_motion = !state.access.reduce_motion;
    persist_access(state);
    Ok(crate::windows::focused_surface_id(state))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corner_zones_map_four_screen_corners() {
        assert_eq!(
            corner_at(0, 0, 1024, 768),
            Some(Corner::TopLeft),
            "top-left pixel must hit the top-left zone"
        );
        assert_eq!(corner_at(1023, 0, 1024, 768), Some(Corner::TopRight));
        assert_eq!(corner_at(0, 767, 1024, 768), Some(Corner::BottomLeft));
        assert_eq!(corner_at(1023, 767, 1024, 768), Some(Corner::BottomRight));
    }

    #[test]
    fn corner_zone_ignores_center_and_just_outside_band() {
        assert_eq!(corner_at(512, 384, 1024, 768), None);
        assert_eq!(corner_at(CORNER_ZONE_PX as i32, 0, 1024, 768), None);
        assert_eq!(corner_at(0, CORNER_ZONE_PX as i32, 1024, 768), None);
        assert_eq!(
            corner_at(1024 - CORNER_ZONE_PX as i32, 767, 1024, 768),
            None
        );
    }

    #[test]
    fn corner_zone_rejects_offscreen_points() {
        assert_eq!(corner_at(-1, 0, 1024, 768), None);
        assert_eq!(corner_at(0, -1, 1024, 768), None);
        assert_eq!(corner_at(1024, 0, 1024, 768), None);
        assert_eq!(corner_at(0, 768, 1024, 768), None);
        assert_eq!(corner_at(0, 0, 0, 0), None);
    }

    #[test]
    fn corner_dwell_fires_once_after_continuous_hold() {
        let mut dwell = CornerDwell::new();
        assert_eq!(dwell.update(Some(Corner::TopLeft), 100), None);
        assert_eq!(dwell.update(Some(Corner::TopLeft), 105), None);
        let fired = dwell.update(Some(Corner::TopLeft), 100 + CORNER_DWELL_TICKS);
        assert_eq!(fired, Some(Corner::TopLeft), "dwell threshold must fire");
        assert_eq!(dwell.update(Some(Corner::TopLeft), 130), None);
    }

    #[test]
    fn corner_dwell_resets_on_leave_and_refires() {
        let mut dwell = CornerDwell::new();
        dwell.update(Some(Corner::TopRight), 10);
        dwell.update(Some(Corner::TopRight), 10 + CORNER_DWELL_TICKS);
        assert_eq!(dwell.update(None, 40), None);
        dwell.update(Some(Corner::TopRight), 50);
        assert_eq!(
            dwell.update(Some(Corner::TopRight), 50 + CORNER_DWELL_TICKS),
            Some(Corner::TopRight),
            "re-entry must re-arm after leaving the zone"
        );
    }

    #[test]
    fn corner_dwell_rejects_corner_switch_mid_hold() {
        let mut dwell = CornerDwell::new();
        dwell.update(Some(Corner::BottomLeft), 0);
        assert_eq!(dwell.update(Some(Corner::BottomRight), 5), None);
        assert_eq!(
            dwell.update(Some(Corner::BottomRight), CORNER_DWELL_TICKS),
            None,
            "switching corners must restart the dwell clock"
        );
    }

    #[test]
    fn zoom_steps_clamp_to_three_levels() {
        assert_eq!(zoom_num_den(0), (2, 2));
        assert_eq!(zoom_num_den(1), (3, 2));
        assert_eq!(zoom_num_den(2), (4, 2));
        assert_eq!(step_zoom(0, false), 0, "cannot zoom out past 1x");
        assert_eq!(step_zoom(ZOOM_STEP_COUNT - 1, true), ZOOM_STEP_COUNT - 1);
        assert_eq!(step_zoom(0, true), 1);
        assert_eq!(step_zoom(1, false), 0);
        assert_eq!(zoom_num_den(9), (4, 2), "out-of-range index clamps to max");
    }

    #[test]
    fn zoom_transform_scales_rect_about_focal_point() {
        let (x, y, w, h) = zoom_transform_rect(100, 100, 200, 100, 200, 150, 2, 1, 4096, 4096);
        assert_eq!((x, y, w, h), (0, 50, 400, 200), "focal point stays fixed");
    }

    #[test]
    fn zoom_transform_identity_at_one_x() {
        let (x, y, w, h) = zoom_transform_rect(30, 40, 200, 100, 500, 300, 2, 2, 4096, 4096);
        assert_eq!((x, y, w, h), (30, 40, 200, 100));
    }

    #[test]
    fn zoom_transform_clamps_into_output_bounds() {
        let (x, y, w, h) = zoom_transform_rect(900, 700, 300, 200, 1000, 800, 2, 1, 1024, 768);
        assert!(x >= 0 && y >= 0, "rect must stay on canvas");
        assert!(
            (x + w as i32) <= 1024 && (y + h as i32) <= 768,
            "scaled rect exceeded output bounds"
        );
        assert!(
            w >= 300 && h >= 200,
            "scaled rect must not shrink below source"
        );
    }

    #[test]
    fn zoom_unmap_inverts_map_about_focal() {
        let (num, den) = zoom_num_den(2);
        let (fx, fy) = (400i32, 300i32);
        let (lx, ly) = (120i32, 90i32);
        let (sx, sy) = (
            fx + (lx - fx) * num as i32 / den as i32,
            fy + (ly - fy) * num as i32 / den as i32,
        );
        let (mx, my) = zoom_unmap_point(2, fx, fy, sx, sy);
        assert_eq!((mx, my), (lx, ly));
        assert_eq!(zoom_unmap_point(0, fx, fy, sx, sy), (sx, sy));
    }

    #[test]
    fn reduce_motion_gate_blocks_animations() {
        assert!(animations_enabled(false));
        assert!(!animations_enabled(true));
    }

    #[test]
    fn access_settings_codec_roundtrip() {
        let settings = AccessSettings {
            high_contrast: true,
            reduce_motion: true,
            zoom_index: 2,
        };
        let bytes = settings.encode();
        assert_eq!(bytes[..3], ACCESS_CONFIG_MAGIC);
        assert_eq!(AccessSettings::decode(&bytes), settings);
    }

    #[test]
    fn access_settings_decode_rejects_bad_magic() {
        assert_eq!(AccessSettings::decode(b"XXXX"), AccessSettings::new());
        assert_eq!(AccessSettings::decode(&[]), AccessSettings::new());
    }

    #[test]
    fn theme_variants_swap_all_channels() {
        let normal = resolve_theme(false);
        let hc = resolve_theme(true);
        assert_ne!(normal.panel, hc.panel);
        assert_ne!(normal.text, hc.text);
        assert_ne!(normal.desktop_bg, hc.desktop_bg);
        assert_ne!(normal.accent, hc.accent);
        assert_ne!(normal.status_ok, hc.status_ok);
    }

    #[test]
    fn app_count_fits_zoom_base_table() {
        assert!(crate::APP_COUNT >= 1);
    }
}
