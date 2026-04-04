use super::*;

pub(crate) fn apply_window_geometry(slot: &AppSlot) -> rt::Result<()> {
    rt::surface_set_geometry(
        slot.window.surface_handle,
        slot.window.x,
        slot.window.y,
        slot.window.width,
        slot.window.height,
        slot.window.z_order,
    )
}

pub(crate) fn sync_window_surface(slot: &AppSlot) -> rt::Result<()> {
    apply_window_geometry(slot)?;
    rt::surface_set_visibility(slot.window.surface_handle, slot.window.visible())
}

pub(crate) fn sync_workspace_visibility(state: &DesktopState) -> rt::Result<()> {
    for slot in state.apps.iter().copied() {
        if !slot.running || slot.window.surface_handle == rt::INVALID_HANDLE {
            continue;
        }
        rt::surface_set_visibility(
            slot.window.surface_handle,
            slot.window.visible() && slot.workspace_id == state.active_workspace,
        )?;
    }
    Ok(())
}

pub(crate) fn visible_on_workspace(state: &DesktopState, app_id: DesktopAppId) -> bool {
    app_slot_index(&state.apps, app_id)
        .map(|index| {
            let slot = state.apps[index];
            slot.running && slot.window.visible() && slot.workspace_id == state.active_workspace
        })
        .unwrap_or(false)
}

pub(crate) fn allocate_z_order(state: &mut DesktopState) -> u32 {
    let z_order = state.next_z_order;
    state.next_z_order = state.next_z_order.saturating_add(1);
    z_order
}

pub(crate) fn focused_surface_id(state: &DesktopState) -> u32 {
    state
        .focused_app
        .and_then(|app_id| app_slot_index(&state.apps, app_id))
        .map(|index| state.apps[index].window.surface_id)
        .unwrap_or(0)
}

pub(crate) fn initial_window_layout(
    output_width: u32,
    app_id: DesktopAppId,
) -> (i32, i32, u32, u32, u32) {
    match app_id {
        DesktopAppId::Settings => (292, 92, 420, 240, ui::BG_WINDOW),
        DesktopAppId::Files => (336, 168, 560, 276, ui::BG_WINDOW_ALT),
        DesktopAppId::Monitor => (
            output_width.saturating_sub(500 + PANEL_MARGIN) as i32,
            108,
            480,
            240,
            ui::BG_WINDOW,
        ),
        DesktopAppId::Terminal => (220, 96, 720, 420, 0x11161f),
        DesktopAppId::SoftwareCenter => (248, 84, 680, 408, ui::BG_WINDOW_ALT),
    }
}

pub(crate) fn clamp_window_x(output_width: u32, width: u32, requested: i32) -> i32 {
    let max_x = output_width.saturating_sub(width + PANEL_MARGIN) as i32;
    requested.clamp(PANEL_MARGIN as i32, max_x.max(PANEL_MARGIN as i32))
}

pub(crate) fn clamp_window_y(output_height: u32, height: u32, requested: i32) -> i32 {
    let min_y = (TOPBAR_HEIGHT + PANEL_MARGIN) as i32;
    let max_y = output_height
        .saturating_sub(height + PANEL_MARGIN)
        .max(TOPBAR_HEIGHT + PANEL_MARGIN) as i32;
    requested.clamp(min_y, max_y)
}

pub(crate) fn launcher_line(slot: AppSlot) -> &'static str {
    match (slot.app_id, slot.running, slot.window.minimized) {
        (DesktopAppId::Settings, true, false) => "SETTINGS  OPEN",
        (DesktopAppId::Settings, true, true) => "SETTINGS  MIN",
        (DesktopAppId::Settings, false, _) => "SETTINGS",
        (DesktopAppId::Files, true, false) => "FILES     OPEN",
        (DesktopAppId::Files, true, true) => "FILES     MIN",
        (DesktopAppId::Files, false, _) => "FILES",
        (DesktopAppId::Monitor, true, false) => "MONITOR   OPEN",
        (DesktopAppId::Monitor, true, true) => "MONITOR   MIN",
        (DesktopAppId::Monitor, false, _) => "MONITOR",
        (DesktopAppId::Terminal, true, false) => "TERMINAL  OPEN",
        (DesktopAppId::Terminal, true, true) => "TERMINAL  MIN",
        (DesktopAppId::Terminal, false, _) => "TERMINAL",
        (DesktopAppId::SoftwareCenter, true, false) => "SOFTWARE  OPEN",
        (DesktopAppId::SoftwareCenter, true, true) => "SOFTWARE  MIN",
        (DesktopAppId::SoftwareCenter, false, _) => "SOFTWARE",
    }
}

pub(crate) fn running_app_count(apps: &[AppSlot; APP_COUNT]) -> usize {
    apps.iter().filter(|slot| slot.running).count()
}

pub(crate) fn app_slot_index(apps: &[AppSlot; APP_COUNT], app_id: DesktopAppId) -> Option<usize> {
    apps.iter().position(|slot| slot.app_id == app_id)
}

pub(crate) fn app_title(app_id: DesktopAppId) -> &'static str {
    match app_id {
        DesktopAppId::Settings => "SETTINGS",
        DesktopAppId::Files => "FILES",
        DesktopAppId::Monitor => "MONITOR",
        DesktopAppId::Terminal => "TERMINAL",
        DesktopAppId::SoftwareCenter => "SOFTWARE",
    }
}
