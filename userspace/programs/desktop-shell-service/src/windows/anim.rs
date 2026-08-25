use super::*;
use crate::{LAUNCHER_WIDTH, PANEL_MARGIN, TOPBAR_HEIGHT};

pub(crate) const ANIM_QUEUE_MAX: usize = 8;
pub(crate) const EASE_SCALE: u32 = 1000;
pub(crate) const ANIM_OPEN_TICKS: u64 = 18;
pub(crate) const ANIM_CLOSE_TICKS: u64 = 15;
pub(crate) const ANIM_MINIMIZE_TICKS: u64 = 22;
pub(crate) const ANIM_RESTORE_TICKS: u64 = 18;
const OPEN_INSET_NUM: u32 = 17;
const OPEN_INSET_DEN: u32 = 20;
const CLOSE_INSET_NUM: u32 = 4;
const CLOSE_INSET_DEN: u32 = 5;
const MINIMIZE_SCALE_NUM: u32 = 11;
const MINIMIZE_SCALE_DEN: u32 = 20;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AnimKind {
    Open,
    Close,
    Minimize,
    Restore,
}

impl AnimKind {
    pub(crate) fn duration_ticks(self) -> u64 {
        match self {
            Self::Open => ANIM_OPEN_TICKS,
            Self::Close => ANIM_CLOSE_TICKS,
            Self::Minimize => ANIM_MINIMIZE_TICKS,
            Self::Restore => ANIM_RESTORE_TICKS,
        }
    }

    pub(crate) fn ease(self, progress: u32) -> u32 {
        match self {
            Self::Open | Self::Restore => ease_out_cubic(progress),
            Self::Close | Self::Minimize => ease_in_cubic(progress),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AnimRect {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WindowAnim {
    pub(crate) app_id: DesktopAppId,
    pub(crate) kind: AnimKind,
    pub(crate) start_tick: u64,
    pub(crate) duration_ticks: u64,
    pub(crate) from: AnimRect,
    pub(crate) to: AnimRect,
}

pub(crate) struct AnimFrame {
    pub(crate) rect: AnimRect,
    pub(crate) done: bool,
}

pub(crate) fn ease_out_cubic(progress: u32) -> u32 {
    let progress = progress.min(EASE_SCALE);
    let inverse = EASE_SCALE - progress;
    EASE_SCALE - inverse * inverse * inverse / (EASE_SCALE * EASE_SCALE)
}

pub(crate) fn ease_in_cubic(progress: u32) -> u32 {
    let progress = progress.min(EASE_SCALE);
    progress * progress * progress / (EASE_SCALE * EASE_SCALE)
}

pub(crate) fn lerp_i32(from: i32, to: i32, eased: u32) -> i32 {
    let eased = eased.min(EASE_SCALE) as i64;
    (from as i64 + ((to as i64 - from as i64) * eased / EASE_SCALE as i64)) as i32
}

pub(crate) fn lerp_u32(from: u32, to: u32, eased: u32) -> u32 {
    let eased = eased.min(EASE_SCALE) as i64;
    (from as i64 + ((to as i64 - from as i64) * eased / EASE_SCALE as i64)) as u32
}

pub(crate) fn anim_rect_at(from: AnimRect, to: AnimRect, eased: u32) -> AnimRect {
    AnimRect {
        x: lerp_i32(from.x, to.x, eased),
        y: lerp_i32(from.y, to.y, eased),
        width: lerp_u32(from.width, to.width, eased),
        height: lerp_u32(from.height, to.height, eased),
    }
}

pub(crate) fn centered_inset_rect(rect: AnimRect, num: u32, den: u32) -> AnimRect {
    let width = (rect.width * num / den).min(rect.width);
    let height = (rect.height * num / den).min(rect.height);
    AnimRect {
        x: rect.x + (rect.width.saturating_sub(width) / 2) as i32,
        y: rect.y + (rect.height.saturating_sub(height) / 2) as i32,
        width,
        height,
    }
}

pub(crate) fn minimize_target_rect(
    rect: AnimRect,
    output_width: u32,
    output_height: u32,
) -> AnimRect {
    let width = (rect.width * MINIMIZE_SCALE_NUM / MINIMIZE_SCALE_DEN).max(1);
    let height = (rect.height * MINIMIZE_SCALE_NUM / MINIMIZE_SCALE_DEN).max(1);
    let dock_x = PANEL_MARGIN as i32 + ((LAUNCHER_WIDTH.saturating_sub(width)) / 2) as i32;
    let dock_y = (output_height.saturating_sub(TOPBAR_HEIGHT + PANEL_MARGIN + height)) as i32;
    AnimRect {
        x: dock_x.clamp(0, output_width.saturating_sub(width) as i32),
        y: dock_y.clamp(0, output_height.saturating_sub(height) as i32),
        width: width.min(output_width.max(1)),
        height: height.min(output_height.max(1)),
    }
}

pub(crate) fn animation_frame(anim: &WindowAnim, now: u64) -> AnimFrame {
    let elapsed = now
        .saturating_sub(anim.start_tick)
        .min(anim.duration_ticks.max(1));
    let progress = ((elapsed * EASE_SCALE as u64) / anim.duration_ticks.max(1)) as u32;
    let eased = anim.kind.ease(progress);
    AnimFrame {
        rect: anim_rect_at(anim.from, anim.to, eased),
        done: elapsed >= anim.duration_ticks,
    }
}

pub(crate) fn queue_push(queue: &mut [Option<WindowAnim>; ANIM_QUEUE_MAX], anim: WindowAnim) {
    for slot in queue.iter_mut() {
        if let Some(existing) = slot {
            if existing.app_id == anim.app_id && existing.kind == anim.kind {
                *slot = Some(anim);
                return;
            }
        }
    }
    if let Some(slot) = queue.iter_mut().find(|slot| slot.is_none()) {
        *slot = Some(anim);
    }
}

pub(crate) fn cancel_animations(
    queue: &mut [Option<WindowAnim>; ANIM_QUEUE_MAX],
    app_id: DesktopAppId,
) {
    for slot in queue.iter_mut() {
        if let Some(existing) = slot {
            if existing.app_id == app_id {
                *slot = None;
            }
        }
    }
}

fn window_anim_rect(window: &WindowState) -> AnimRect {
    AnimRect {
        x: window.x,
        y: window.y,
        width: window.width,
        height: window.height,
    }
}

fn begin_animation(
    state: &mut DesktopState,
    app_id: DesktopAppId,
    kind: AnimKind,
    from: AnimRect,
    to: AnimRect,
) -> rt::Result<()> {
    let now = rt::monotonic_now()?;
    if !crate::access::animations_enabled(state.access.reduce_motion) {
        return Ok(());
    }
    queue_push(
        &mut state.animations,
        WindowAnim {
            app_id,
            kind,
            start_tick: now,
            duration_ticks: kind.duration_ticks(),
            from,
            to,
        },
    );
    Ok(())
}

pub(crate) fn begin_open_animation(
    state: &mut DesktopState,
    app_id: DesktopAppId,
) -> rt::Result<()> {
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Ok(());
    };
    if !state.apps[index].running || state.apps[index].window.surface_handle == rt::INVALID_HANDLE {
        return Ok(());
    }
    let rect = window_anim_rect(&state.apps[index].window);
    let from = centered_inset_rect(rect, OPEN_INSET_NUM, OPEN_INSET_DEN);
    begin_animation(state, app_id, AnimKind::Open, from, rect)
}

pub(crate) fn begin_close_animation(
    state: &mut DesktopState,
    app_id: DesktopAppId,
) -> rt::Result<()> {
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Ok(());
    };
    if !state.apps[index].running || state.apps[index].window.surface_handle == rt::INVALID_HANDLE {
        return Ok(());
    }
    let rect = window_anim_rect(&state.apps[index].window);
    let to = centered_inset_rect(rect, CLOSE_INSET_NUM, CLOSE_INSET_DEN);
    begin_animation(state, app_id, AnimKind::Close, rect, to)
}

pub(crate) fn begin_minimize_animation(
    state: &mut DesktopState,
    app_id: DesktopAppId,
) -> rt::Result<()> {
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Ok(());
    };
    if !state.apps[index].running || state.apps[index].window.surface_handle == rt::INVALID_HANDLE {
        return Ok(());
    }
    let rect = window_anim_rect(&state.apps[index].window);
    let to = minimize_target_rect(rect, state.chrome.output_width, state.chrome.output_height);
    begin_animation(state, app_id, AnimKind::Minimize, rect, to)
}

pub(crate) fn begin_restore_animation(
    state: &mut DesktopState,
    app_id: DesktopAppId,
) -> rt::Result<()> {
    let Some(index) = app_slot_index(&state.apps, app_id) else {
        return Ok(());
    };
    if !state.apps[index].running || state.apps[index].window.surface_handle == rt::INVALID_HANDLE {
        return Ok(());
    }
    let rect = window_anim_rect(&state.apps[index].window);
    let from = minimize_target_rect(rect, state.chrome.output_width, state.chrome.output_height);
    begin_animation(state, app_id, AnimKind::Restore, from, rect)
}

pub(crate) fn step_animations(state: &mut DesktopState, now: u64) {
    for index in 0..state.animations.len() {
        let Some(anim) = state.animations[index] else {
            continue;
        };
        let Some(app_index) = app_slot_index(&state.apps, anim.app_id) else {
            state.animations[index] = None;
            continue;
        };
        let slot = &state.apps[app_index];
        let stale = !slot.running
            || slot.window.surface_handle == rt::INVALID_HANDLE
            || slot.workspace_id != state.active_workspace
            || (slot.window.minimized && anim.kind != AnimKind::Minimize);
        if stale {
            state.animations[index] = None;
            continue;
        }
        let frame = animation_frame(&anim, now);
        let handle = slot.window.surface_handle;
        let z_order = slot.window.z_order;
        let _ = rt::surface_set_geometry_async(
            handle,
            frame.rect.x,
            frame.rect.y,
            frame.rect.width,
            frame.rect.height,
            z_order,
        );
        if frame.done {
            match anim.kind {
                AnimKind::Minimize => {
                    let _ = rt::surface_set_visibility(handle, false);
                }
                AnimKind::Open | AnimKind::Restore => {
                    let _ = rt::surface_set_geometry_async(
                        handle,
                        anim.to.x,
                        anim.to.y,
                        anim.to.width,
                        anim.to.height,
                        z_order,
                    );
                }
                AnimKind::Close => {}
            }
            state.animations[index] = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, width: u32, height: u32) -> AnimRect {
        AnimRect {
            x,
            y,
            width,
            height,
        }
    }

    fn anim(kind: AnimKind, start: u64, from: AnimRect, to: AnimRect) -> WindowAnim {
        WindowAnim {
            app_id: DesktopAppId::Settings,
            kind,
            start_tick: start,
            duration_ticks: kind.duration_ticks(),
            from,
            to,
        }
    }

    #[test]
    fn easing_endpoints_match_and_stay_monotonic() {
        for kind in [
            AnimKind::Open,
            AnimKind::Close,
            AnimKind::Minimize,
            AnimKind::Restore,
        ] {
            assert_eq!(kind.ease(0), 0);
            assert_eq!(kind.ease(EASE_SCALE), EASE_SCALE);
            let mut previous = 0u32;
            for step in 0..=EASE_SCALE {
                let value = kind.ease(step);
                assert!(value >= previous);
                assert!(value <= EASE_SCALE);
                previous = value;
            }
        }
    }

    #[test]
    fn ease_out_leads_and_ease_in_lags_linear_midpoint() {
        assert!(ease_out_cubic(EASE_SCALE / 2) > EASE_SCALE / 2);
        assert!(ease_in_cubic(EASE_SCALE / 2) < EASE_SCALE / 2);
        assert_eq!(ease_out_cubic(u32::MAX), EASE_SCALE);
        assert_eq!(ease_in_cubic(u32::MAX), EASE_SCALE);
    }

    #[test]
    fn lerp_hits_exact_endpoints_and_midpoint() {
        assert_eq!(lerp_i32(-40, 60, 0), -40);
        assert_eq!(lerp_i32(-40, 60, EASE_SCALE), 60);
        assert_eq!(lerp_i32(-40, 60, EASE_SCALE / 2), 10);
        assert_eq!(lerp_u32(100, 200, 0), 100);
        assert_eq!(lerp_u32(100, 200, EASE_SCALE), 200);
        assert_eq!(lerp_u32(100, 200, EASE_SCALE / 2), 150);
        assert_eq!(lerp_i32(30, 10, EASE_SCALE), 10);
    }

    #[test]
    fn animation_frame_interpolates_then_clamps_done() {
        let subject = anim(
            AnimKind::Open,
            100,
            rect(0, 0, 100, 100),
            rect(50, 50, 200, 200),
        );
        let halfway = animation_frame(&subject, 100 + ANIM_OPEN_TICKS / 2);
        assert!(!halfway.done);
        assert!(halfway.rect.width > 100 && halfway.rect.width < 200);
        let finished = animation_frame(&subject, 100 + ANIM_OPEN_TICKS * 10);
        assert!(finished.done);
        assert_eq!(finished.rect, rect(50, 50, 200, 200));
        let before_start = animation_frame(&subject, 0);
        assert_eq!(before_start.rect, rect(0, 0, 100, 100));
        assert!(!before_start.done);
    }

    #[test]
    fn centered_inset_rect_shrinks_around_center() {
        let subject = rect(100, 200, 400, 160);
        let inset = centered_inset_rect(subject, 17, 20);
        assert_eq!(inset.width, 340);
        assert_eq!(inset.height, 136);
        assert_eq!(inset.x, 130);
        assert_eq!(inset.y, 212);
        assert_eq!(centered_inset_rect(subject, 20, 20), subject);
    }

    #[test]
    fn minimize_target_rect_stays_inside_output() {
        let window = rect(600, 80, 720, 420);
        let target = minimize_target_rect(window, 1024, 768);
        assert!(target.x >= 0);
        assert!(target.y >= 0);
        assert!(target.x + target.width as i32 <= 1024);
        assert!(target.y + target.height as i32 <= 768);
        assert!(target.width < window.width);
        assert!(target.height < window.height);
        let tiny_output = minimize_target_rect(window, 320, 200);
        assert!(tiny_output.x >= 0 && tiny_output.y >= 0);
        assert!(tiny_output.width <= 320 && tiny_output.height <= 200);
    }

    #[test]
    fn queue_replaces_matching_kind_and_caps_without_growth() {
        let mut queue = [None; ANIM_QUEUE_MAX];
        let open = anim(AnimKind::Open, 0, rect(0, 0, 1, 1), rect(1, 1, 2, 2));
        assert!(queue.iter().all(|slot| slot.is_none()));
        queue_push(&mut queue, open);
        let replacement = anim(AnimKind::Open, 5, rect(0, 0, 1, 1), rect(2, 2, 3, 3));
        queue_push(&mut queue, replacement);
        let total = queue.iter().filter(|slot| slot.is_some()).count();
        assert_eq!(total, 1);
        assert_eq!(queue[0], Some(replacement));
        let fill_apps = [
            DesktopAppId::Files,
            DesktopAppId::Monitor,
            DesktopAppId::Terminal,
            DesktopAppId::SoftwareCenter,
        ];
        for index in 0..(ANIM_QUEUE_MAX - 1) {
            let mut entry = anim(
                if index < fill_apps.len() {
                    AnimKind::Close
                } else {
                    AnimKind::Minimize
                },
                10 + index as u64,
                rect(0, 0, 1, 1),
                rect(1, 1, 1, 1),
            );
            entry.app_id = fill_apps[index % fill_apps.len()];
            queue_push(&mut queue, entry);
        }
        assert_eq!(
            queue.iter().filter(|slot| slot.is_some()).count(),
            ANIM_QUEUE_MAX
        );
        let overflow = anim(AnimKind::Restore, 99, rect(0, 0, 1, 1), rect(1, 1, 1, 1));
        queue_push(&mut queue, overflow);
        assert_eq!(
            queue.iter().filter(|slot| slot.is_some()).count(),
            ANIM_QUEUE_MAX
        );
        assert!(
            queue
                .iter()
                .all(|slot| slot.unwrap().kind != AnimKind::Restore)
        );
    }

    #[test]
    fn cancel_removes_only_target_app_entries() {
        let mut queue = [None; ANIM_QUEUE_MAX];
        queue_push(
            &mut queue,
            anim(AnimKind::Open, 0, rect(0, 0, 1, 1), rect(1, 1, 1, 1)),
        );
        let mut files_open = anim(AnimKind::Open, 1, rect(0, 0, 1, 1), rect(1, 1, 1, 1));
        files_open.app_id = DesktopAppId::Files;
        queue_push(&mut queue, files_open);
        cancel_animations(&mut queue, DesktopAppId::Settings);
        let remaining: Vec<DesktopAppId> = queue
            .iter()
            .filter_map(|slot| slot.map(|entry| entry.app_id))
            .collect();
        assert_eq!(remaining, vec![DesktopAppId::Files]);
        cancel_animations(&mut queue, DesktopAppId::Files);
        assert!(queue.iter().all(|slot| slot.is_none()));
    }
}
