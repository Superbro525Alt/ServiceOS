use serviceos_userspace_runtime as rt;

use rt::DesktopAppId;

use crate::{
    KEY_1, KEY_2, KEY_3, KEY_4, KEY_5, KEY_A, KEY_EQUAL, KEY_F, KEY_H, KEY_J, KEY_L, KEY_LEFT,
    KEY_M, KEY_MINUS, KEY_N, KEY_RIGHT, KEY_SPACE, KEY_UP, KEY_V, MOD_ALT, MOD_CTRL, MOD_SHIFT,
    OverlayMode, PaletteAction, WORKSPACE_COUNT, access::Corner,
};

/// One registry row: a shell action plus every surface that can trigger it
/// (palette text, global key binding, hot corner, quick-action key).
/// Registering a new action = one enum variant + one row here.
pub(crate) struct ActionEntry {
    pub(crate) action: PaletteAction,
    pub(crate) label: &'static str,
    pub(crate) keywords: &'static str,
    pub(crate) binding: Option<(u32, u32)>,
    pub(crate) corner: Option<Corner>,
    pub(crate) quick_key: Option<u32>,
}

const fn action(act: PaletteAction, label: &'static str, keywords: &'static str) -> ActionEntry {
    ActionEntry {
        action: act,
        label,
        keywords,
        binding: None,
        corner: None,
        quick_key: None,
    }
}

const fn bind(
    act: PaletteAction,
    label: &'static str,
    keywords: &'static str,
    mods: u32,
    key: u32,
) -> ActionEntry {
    ActionEntry {
        binding: Some((mods, key)),
        ..action(act, label, keywords)
    }
}

const fn corner(
    act: PaletteAction,
    label: &'static str,
    keywords: &'static str,
    which: Corner,
) -> ActionEntry {
    ActionEntry {
        corner: Some(which),
        ..action(act, label, keywords)
    }
}

const fn quick(
    act: PaletteAction,
    label: &'static str,
    keywords: &'static str,
    key: u32,
) -> ActionEntry {
    ActionEntry {
        quick_key: Some(key),
        ..action(act, label, keywords)
    }
}

pub(crate) const ACTION_MAX: usize = 40;

/// Single source of truth for shell actions across palette, hot corners,
/// quick actions, and keyboard shortcuts.
pub(crate) const REGISTRY: [ActionEntry; 36] = [
    bind(
        PaletteAction::Launch(DesktopAppId::Settings),
        "Open Settings",
        "settings preferences",
        MOD_ALT,
        KEY_1,
    ),
    bind(
        PaletteAction::Launch(DesktopAppId::Files),
        "Open Files",
        "files browser",
        MOD_ALT,
        KEY_2,
    ),
    bind(
        PaletteAction::Launch(DesktopAppId::Monitor),
        "Open Monitor",
        "monitor system health",
        MOD_ALT,
        KEY_3,
    ),
    bind(
        PaletteAction::Launch(DesktopAppId::Terminal),
        "Open Terminal",
        "terminal console shell",
        MOD_ALT,
        KEY_4,
    ),
    bind(
        PaletteAction::Launch(DesktopAppId::SoftwareCenter),
        "Open Software Center",
        "software packages store",
        MOD_ALT,
        KEY_5,
    ),
    ActionEntry {
        binding: Some((MOD_ALT, KEY_N)),
        corner: Some(Corner::TopRight),
        ..action(
            PaletteAction::ShowNotifications,
            "Show Notification History",
            "notifications history",
        )
    },
    action(
        PaletteAction::ToggleNotifications,
        "Toggle Notification History",
        "notifications toggle",
    ),
    quick(
        PaletteAction::DismissAllNotifications,
        "Dismiss All Notifications",
        "dismiss clear notifications",
        KEY_A,
    ),
    quick(
        PaletteAction::FocusNotificationSource,
        "Focus Notification Source",
        "focus source notification",
        KEY_F,
    ),
    bind(
        PaletteAction::ShowClipboardHistory,
        "Show Clipboard History",
        "clipboard history paste",
        MOD_CTRL | MOD_ALT,
        KEY_V,
    ),
    action(
        PaletteAction::ToggleClipboardHistory,
        "Toggle Clipboard History",
        "clipboard toggle",
    ),
    action(
        PaletteAction::ShowMedia,
        "Show Media and Volume",
        "media volume sound",
    ),
    bind(
        PaletteAction::ToggleMedia,
        "Toggle Media and Volume",
        "media volume toggle",
        MOD_ALT,
        KEY_M,
    ),
    action(
        PaletteAction::CycleSettingsPage,
        "Cycle Settings Page",
        "settings page cycle",
    ),
    action(
        PaletteAction::LockSession,
        "Lock Session",
        "lock session security",
    ),
    bind(
        PaletteAction::ShowLogin,
        "Login",
        "login account sign-in session credentials",
        MOD_ALT,
        KEY_L,
    ),
    bind(
        ws(1),
        "Switch to Workspace 1",
        "workspace one",
        MOD_CTRL | MOD_ALT,
        KEY_1,
    ),
    bind(
        ws(2),
        "Switch to Workspace 2",
        "workspace two",
        MOD_CTRL | MOD_ALT,
        KEY_2,
    ),
    bind(
        ws(3),
        "Switch to Workspace 3",
        "workspace three",
        MOD_CTRL | MOD_ALT,
        KEY_3,
    ),
    bind(
        ws(4),
        "Switch to Workspace 4",
        "workspace four",
        MOD_CTRL | MOD_ALT,
        KEY_4,
    ),
    bind(
        mws(1),
        "Move Window to Workspace 1",
        "move window workspace one",
        MOD_CTRL | MOD_ALT | MOD_SHIFT,
        KEY_1,
    ),
    bind(
        mws(2),
        "Move Window to Workspace 2",
        "move window workspace two",
        MOD_CTRL | MOD_ALT | MOD_SHIFT,
        KEY_2,
    ),
    bind(
        mws(3),
        "Move Window to Workspace 3",
        "move window workspace three",
        MOD_CTRL | MOD_ALT | MOD_SHIFT,
        KEY_3,
    ),
    bind(
        mws(4),
        "Move Window to Workspace 4",
        "move window workspace four",
        MOD_CTRL | MOD_ALT | MOD_SHIFT,
        KEY_4,
    ),
    action(
        PaletteAction::FocusNext,
        "Focus Next Window",
        "focus next window cycle",
    ),
    corner(
        PaletteAction::OpenTaskSwitcher,
        "Task Switcher",
        "switcher tasks mru alt tab",
        Corner::TopLeft,
    ),
    corner(
        PaletteAction::OpenCommandPalette,
        "Command Palette",
        "palette launcher commands search",
        Corner::BottomLeft,
    ),
    corner(
        PaletteAction::ToggleShowDesktop,
        "Show Desktop",
        "show desktop minimize restore",
        Corner::BottomRight,
    ),
    bind(
        PaletteAction::ToggleWorkspaceOverview,
        "Workspace Overview",
        "workspaces overview mission control expose",
        MOD_CTRL | MOD_ALT,
        KEY_UP,
    ),
    bind(
        PaletteAction::SnapFocusedLeft,
        "Snap Window Left",
        "snap window left half tiling",
        MOD_CTRL | MOD_ALT,
        KEY_LEFT,
    ),
    bind(
        PaletteAction::SnapFocusedRight,
        "Snap Window Right",
        "snap window right half tiling",
        MOD_CTRL | MOD_ALT,
        KEY_RIGHT,
    ),
    action(
        PaletteAction::MinimizeFocused,
        "Minimize Window",
        "minimize window hide",
    ),
    bind(
        PaletteAction::ZoomIn,
        "Magnifier Zoom In",
        "zoom magnifier larger",
        MOD_CTRL | MOD_ALT,
        KEY_EQUAL,
    ),
    bind(
        PaletteAction::ZoomOut,
        "Magnifier Zoom Out",
        "zoom magnifier smaller",
        MOD_CTRL | MOD_ALT,
        KEY_MINUS,
    ),
    bind(
        PaletteAction::ToggleHighContrast,
        "High Contrast Mode",
        "high contrast theme accessibility",
        MOD_CTRL | MOD_ALT,
        KEY_H,
    ),
    bind(
        PaletteAction::ToggleReduceMotion,
        "Reduce Motion",
        "reduce motion animation accessibility",
        MOD_CTRL | MOD_ALT,
        KEY_J,
    ),
];

const fn ws(id: u32) -> PaletteAction {
    PaletteAction::SwitchWorkspace(id)
}

const fn mws(id: u32) -> PaletteAction {
    PaletteAction::MoveFocusedToWorkspace(id)
}

pub(crate) fn lookup(target: PaletteAction) -> Option<&'static ActionEntry> {
    REGISTRY.iter().find(|entry| entry.action == target)
}

/// Label for any registered action; unregistered variants fall back here so
/// app-launch ids stay renderable even when absent from the catalog.
pub(crate) fn action_label(action: PaletteAction) -> &'static str {
    if let Some(entry) = lookup(action) {
        return entry.label;
    }
    match action {
        PaletteAction::Launch(DesktopAppId::Media) => "Open Media",
        _ => "",
    }
}

/// Exact-modifier global shortcut lookup (Ctrl+Alt+1 differs from Alt+1).
pub(crate) fn action_for_binding(modifiers: u32, key_code: u32) -> Option<PaletteAction> {
    REGISTRY
        .iter()
        .find(|entry| {
            matches!(
                entry.binding,
                Some((mods, key)) if mods == modifiers && key == key_code
            )
        })
        .map(|entry| entry.action)
}

pub(crate) fn action_for_corner(which: Corner) -> Option<PaletteAction> {
    REGISTRY
        .iter()
        .find(|entry| entry.corner == Some(which))
        .map(|entry| entry.action)
}

pub(crate) fn action_for_quick_key(key_code: u32) -> Option<PaletteAction> {
    REGISTRY
        .iter()
        .find(|entry| entry.quick_key == Some(key_code))
        .map(|entry| entry.action)
}

/// Palette catalog: every registered action that carries a user-visible label.
pub(crate) fn palette_catalog() -> ([PaletteAction; ACTION_MAX], usize) {
    let mut actions = [PaletteAction::ShowNotifications; ACTION_MAX];
    let mut count = 0usize;
    for entry in REGISTRY.iter() {
        if count >= ACTION_MAX {
            break;
        }
        actions[count] = entry.action;
        count += 1;
    }
    (actions, count)
}

fn open_task_switcher(state: &mut crate::DesktopState) {
    let model = crate::switcher::switcher_model(state);
    state.overlay_mode = OverlayMode::Switcher;
    state.switcher_selection = crate::switcher::open_selection(&model, state.focused_app);
    state.overlay_selection = 0;
}

fn toggle_workspace_overview(state: &mut crate::DesktopState) {
    if state.overlay_mode == OverlayMode::WorkspaceOverview {
        state.overlay_mode = OverlayMode::None;
        state.overlay_selection = 0;
    } else {
        state.overlay_mode = OverlayMode::WorkspaceOverview;
        state.overlay_selection = state.active_workspace.saturating_sub(1) as usize;
    }
}

/// Executes any registered shell action. Render-free by contract: callers
/// render via the input dispatcher's diff detection or their own path.
pub(crate) fn execute_shell_action(
    state: &mut crate::DesktopState,
    action: PaletteAction,
) -> rt::Result<u32> {
    match action {
        PaletteAction::Launch(app_id) => {
            crate::windows::schedule_launch_or_focus_app(state, app_id)
        }
        PaletteAction::ShowNotifications => {
            state.overlay_mode = OverlayMode::Notifications;
            state.overlay_selection = 0;
            Ok(crate::windows::focused_surface_id(state))
        }
        PaletteAction::ToggleNotifications => {
            state.overlay_mode = toggle(state.overlay_mode, OverlayMode::Notifications);
            state.overlay_selection = 0;
            Ok(crate::windows::focused_surface_id(state))
        }
        PaletteAction::ShowClipboardHistory => {
            state.overlay_mode = OverlayMode::ClipboardHistory;
            state.overlay_selection = 0;
            Ok(crate::windows::focused_surface_id(state))
        }
        PaletteAction::ToggleClipboardHistory => {
            state.overlay_mode = toggle(state.overlay_mode, OverlayMode::ClipboardHistory);
            state.overlay_selection = 0;
            Ok(crate::windows::focused_surface_id(state))
        }
        PaletteAction::ShowMedia => {
            state.overlay_mode = OverlayMode::Media;
            state.overlay_selection = 0;
            Ok(crate::windows::focused_surface_id(state))
        }
        PaletteAction::ToggleMedia => {
            state.overlay_mode = toggle(state.overlay_mode, OverlayMode::Media);
            state.overlay_selection = 0;
            Ok(crate::windows::focused_surface_id(state))
        }
        PaletteAction::DismissAllNotifications => {
            crate::input::overlays::dismiss_all_notifications_now(state)?;
            Ok(crate::windows::focused_surface_id(state))
        }
        PaletteAction::FocusNotificationSource => {
            crate::input::overlays::focus_notification_source(state)
        }
        PaletteAction::CycleSettingsPage => cycle_settings_page(state),
        PaletteAction::LockSession => lock_session_stub(state),
        PaletteAction::ShowLogin => crate::login::open_login_overlay(state),
        PaletteAction::SwitchWorkspace(workspace_id) => {
            crate::windows::switch_workspace(state, workspace_id)
        }
        PaletteAction::MoveFocusedToWorkspace(workspace_id) => {
            crate::windows::move_focused_to_workspace(state, workspace_id)
        }
        PaletteAction::FocusNext => crate::input::focus_next_app(state),
        PaletteAction::OpenTaskSwitcher => {
            open_task_switcher(state);
            Ok(crate::windows::focused_surface_id(state))
        }
        PaletteAction::OpenCommandPalette => {
            state.overlay_mode = OverlayMode::CommandPalette;
            state.overlay_selection = 0;
            state.palette_query_len = 0;
            crate::palette_docs::refresh_doc_hits(state);
            Ok(crate::windows::focused_surface_id(state))
        }
        PaletteAction::ToggleShowDesktop => {
            crate::input::toggle_show_desktop(state)?;
            Ok(crate::windows::focused_surface_id(state))
        }
        PaletteAction::ToggleWorkspaceOverview => {
            toggle_workspace_overview(state);
            Ok(crate::windows::focused_surface_id(state))
        }
        PaletteAction::SnapFocusedLeft | PaletteAction::SnapFocusedRight => {
            let Some(app_id) = state.focused_app else {
                return Ok(crate::windows::focused_surface_id(state));
            };
            crate::windows::snap_window_half(
                state,
                app_id,
                action == PaletteAction::SnapFocusedLeft,
            )
        }
        PaletteAction::MinimizeFocused => {
            let Some(app_id) = state.focused_app else {
                return Ok(crate::windows::focused_surface_id(state));
            };
            crate::windows::minimize_app(state, app_id)
        }
        PaletteAction::ZoomIn => crate::access::apply_zoom_step(state, true),
        PaletteAction::ZoomOut => crate::access::apply_zoom_step(state, false),
        PaletteAction::ToggleHighContrast => crate::access::toggle_high_contrast(state),
        PaletteAction::ToggleReduceMotion => crate::access::toggle_reduce_motion(state),
    }
}

fn toggle(current: OverlayMode, target: OverlayMode) -> OverlayMode {
    if current == target {
        OverlayMode::None
    } else {
        target
    }
}

fn cycle_settings_page(state: &mut crate::DesktopState) -> rt::Result<u32> {
    let index = crate::windows::app_slot_index(&state.apps, DesktopAppId::Settings)
        .ok_or(rt::Error::NotFound)?;
    if state.apps[index].running && state.apps[index].window.control_handle != rt::INVALID_HANDLE {
        let control = state.apps[index].window.control_handle;
        rt::app_control_key(control, rt::AppKeyAction::Down, crate::KEY_TAB, 0)?;
        rt::app_control_key(control, rt::AppKeyAction::Up, crate::KEY_TAB, 0)?;
        return crate::windows::focus_app(state, DesktopAppId::Settings);
    }
    crate::windows::schedule_launch_or_focus_app(state, DesktopAppId::Settings)
}

fn lock_session_stub(state: &mut crate::DesktopState) -> rt::Result<u32> {
    state.overlay_mode = OverlayMode::None;
    state.overlay_selection = 0;
    state.switcher_selection = 0;
    state.palette_query_len = 0;
    crate::windows::post_notification(state, None, false, false, b"SESSION LOCKED (SHELL STUB)")?;
    Ok(crate::windows::focused_surface_id(state))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{KEY_DOWN, KEY_TAB};

    #[test]
    fn login_binding_does_not_collide_with_workspace_shortcuts() {
        // Alt+L belongs to the login overlay; Ctrl+Alt+1..4 stay workspace keys.
        assert_eq!(
            action_for_binding(MOD_ALT, crate::KEY_L),
            Some(PaletteAction::ShowLogin)
        );
        assert_ne!(
            action_for_binding(MOD_ALT, crate::KEY_L),
            action_for_binding(MOD_CTRL | MOD_ALT, crate::KEY_1)
        );
    }

    #[test]
    fn every_corner_is_bound_exactly_once() {
        let kinds = [
            Corner::TopLeft,
            Corner::TopRight,
            Corner::BottomLeft,
            Corner::BottomRight,
        ];
        for kind in kinds {
            let hits = REGISTRY
                .iter()
                .filter(|entry| entry.corner == Some(kind))
                .count();
            assert_eq!(hits, 1, "corner {kind:?} bound {hits} times");
            assert!(action_for_corner(kind).is_some());
        }
    }

    #[test]
    fn bindings_are_unique_and_resolve() {
        for (index, entry) in REGISTRY.iter().enumerate() {
            let Some((mods, key)) = entry.binding else {
                continue;
            };
            assert!(!entry.label.is_empty(), "bound action lacks label");
            for prev in REGISTRY.iter().take(index) {
                if let Some((prev_mods, prev_key)) = prev.binding {
                    assert!(
                        !(prev_mods == mods && prev_key == key),
                        "duplicate binding {mods:#b}+{key}"
                    );
                }
            }
            let resolved = action_for_binding(mods, key).expect("binding resolves");
            assert_eq!(action_label(resolved), entry.label);
        }
        assert_eq!(
            action_for_binding(MOD_CTRL | MOD_ALT, KEY_UP),
            Some(PaletteAction::ToggleWorkspaceOverview)
        );
        assert_eq!(
            action_for_binding(MOD_CTRL | MOD_ALT, KEY_LEFT),
            Some(PaletteAction::SnapFocusedLeft)
        );
        assert_eq!(
            action_for_binding(MOD_CTRL | MOD_ALT | MOD_SHIFT, KEY_4),
            Some(PaletteAction::MoveFocusedToWorkspace(4))
        );
        // Modifier sets match exactly: plain Alt+1 launches, Ctrl+Alt+1 switches.
        assert_ne!(
            action_for_binding(MOD_ALT, KEY_1),
            action_for_binding(MOD_CTRL | MOD_ALT, KEY_1)
        );
    }

    #[test]
    fn unregistered_keys_miss_the_registry() {
        assert_eq!(action_for_binding(MOD_CTRL, KEY_TAB), None);
        assert_eq!(action_for_binding(0, KEY_DOWN), None);
        assert_eq!(action_for_binding(MOD_ALT, KEY_SPACE), None);
    }

    #[test]
    fn quick_action_keys_map_through_registry() {
        assert_eq!(
            action_for_quick_key(KEY_A),
            Some(PaletteAction::DismissAllNotifications)
        );
        assert_eq!(
            action_for_quick_key(KEY_F),
            Some(PaletteAction::FocusNotificationSource)
        );
        assert_eq!(action_for_quick_key(KEY_TAB), None);
    }

    #[test]
    fn palette_catalog_covers_core_actions_with_labels() {
        let (actions, count) = palette_catalog();
        assert!(count >= 30, "catalog shrank: {count}");
        assert!(count <= ACTION_MAX);
        let expected = [
            PaletteAction::LockSession,
            PaletteAction::ToggleWorkspaceOverview,
            PaletteAction::SnapFocusedLeft,
            PaletteAction::MinimizeFocused,
            PaletteAction::FocusNext,
        ];
        for want in expected {
            assert!(actions[..count].contains(&want), "missing {want:?}");
        }
        for index in 0..count {
            assert!(!action_label(actions[index]).is_empty());
        }
    }

    #[test]
    fn labels_are_unique_across_registry() {
        for i in 0..REGISTRY.len() {
            for j in (i + 1)..REGISTRY.len() {
                assert_ne!(
                    REGISTRY[i].label, REGISTRY[j].label,
                    "duplicate label at {i}/{j}"
                );
            }
        }
    }

    #[test]
    fn workspace_actions_stay_within_workspace_count() {
        for entry in REGISTRY.iter() {
            if let PaletteAction::SwitchWorkspace(id) | PaletteAction::MoveFocusedToWorkspace(id) =
                entry.action
            {
                assert!(id >= 1 && id <= WORKSPACE_COUNT);
            }
        }
    }
}
