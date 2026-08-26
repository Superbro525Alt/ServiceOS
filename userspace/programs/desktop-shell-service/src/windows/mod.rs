mod anim;
mod drag;
mod encode;
mod gestures;
mod layout;
mod lifecycle;
mod notifications;
mod shadow;

use core::fmt::Write;

use rt::{DesktopAppId, FixedLogBuffer, LogEvent, LogSeverity, StartupHandle};
use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;

use crate::{
    APP_COUNT, AppSlot, DesktopState, MAX_NOTIFICATION_BYTES, NOTIFICATION_HISTORY_MAX,
    NOTIFICATION_HISTORY_TEXT_MAX, NOTIFICATION_TIMEOUT_TICKS, NotificationEntry, OverlayMode,
    PANEL_MARGIN, SESSION_ID, TOPBAR_HEIGHT, WINDOW_MIN_HEIGHT, WINDOW_MIN_WIDTH, WORKSPACE_COUNT,
    WindowState,
    logging::{emit_log, emit_text_log},
    render::render_desktop,
};

pub(crate) use anim::{
    ANIM_QUEUE_MAX, WindowAnim, begin_close_animation, begin_minimize_animation,
    begin_open_animation, begin_restore_animation, cancel_animations, step_animations,
};
pub(crate) use drag::{
    CONTENT_DRAG_TIMEOUT_TICKS, ContentDrag, DropDecision, drop_decision, parse_content_intent,
};
pub(crate) use encode::{encode_window_page, pack_i32_pair};
pub(crate) use gestures::{
    SnapZone, hide_snap_preview, snap_zone_at, step_workspace_selection, overview_tile_at,
    overview_tile_rect, update_snap_preview,
};
pub(crate) use layout::{
    allocate_z_order, app_slot_index, app_title, apply_window_geometry,
    apply_window_geometry_async, clamp_window_x, clamp_window_y, focused_surface_id,
    initial_window_layout, launcher_line, running_app_count, set_window_visibility,
    sync_workspace_visibility, visible_on_workspace,
};
pub(crate) use lifecycle::{
    close_app, deliver_open_intent, flush_pending_resize, focus_app, launch_or_focus_app,
    maximize_app, minimize_app, move_app, move_focused_to_workspace, open_path_in_files,
    refresh_apps, resize_app, restore_app, schedule_launch_or_focus_app, snap_window_half,
    switch_workspace,
};
#[cfg(test)]
pub(crate) use notifications::mru_promote;
pub(crate) use notifications::{
    dismiss_all_notifications, dismiss_selected_notification, post_notification,
    push_recent_focus,
};
pub(crate) use shadow::sync_focus_shadow;
