use serviceos_userspace_runtime::DesktopAppId;

use crate::{
    state::{AppSlot, OVERLAY_RESULT_MAX, PaletteAction, PALETTE_ACTION_MAX, APP_COUNT},
    windows,
};

pub(crate) fn palette_matches(
    state: &crate::DesktopState,
    results: &mut [PaletteAction; OVERLAY_RESULT_MAX],
) -> usize {
    let query =
        core::str::from_utf8(&state.palette_query[..state.palette_query_len]).unwrap_or("");
    rank_palette(
        &state.apps,
        state.focused_app,
        &state.recent_focus,
        state.recent_focus_len,
        query,
        results,
    )
}

pub(crate) fn rank_palette(
    apps: &[AppSlot; APP_COUNT],
    focused_app: Option<DesktopAppId>,
    recent_focus: &[DesktopAppId],
    recent_focus_len: usize,
    query: &str,
    results: &mut [PaletteAction; OVERLAY_RESULT_MAX],
) -> usize {
    let actions = [
        PaletteAction::Launch(DesktopAppId::Settings),
        PaletteAction::Launch(DesktopAppId::Files),
        PaletteAction::Launch(DesktopAppId::Monitor),
        PaletteAction::Launch(DesktopAppId::Terminal),
        PaletteAction::Launch(DesktopAppId::SoftwareCenter),
        PaletteAction::ShowNotifications,
        PaletteAction::ToggleNotifications,
        PaletteAction::DismissAllNotifications,
        PaletteAction::ShowClipboardHistory,
        PaletteAction::ToggleClipboardHistory,
        PaletteAction::ShowMedia,
        PaletteAction::ToggleMedia,
        PaletteAction::CycleSettingsPage,
        PaletteAction::LockSession,
        PaletteAction::SwitchWorkspace(1),
        PaletteAction::SwitchWorkspace(2),
        PaletteAction::SwitchWorkspace(3),
        PaletteAction::SwitchWorkspace(4),
        PaletteAction::FocusNext,
    ];
    let mut ranked = [(PaletteAction::ShowNotifications, 0u32); PALETTE_ACTION_MAX];
    let mut ranked_len = 0usize;
    for action in actions {
        let label = palette_action_label(action);
        let matches = query.is_empty() || contains_case_fold(label, query);
        if !matches {
            continue;
        }
        let mut score = 1u32;
        if query.is_empty() {
            score = score.saturating_add(1);
        }
        if starts_with_case_fold(label, query) {
            score = score.saturating_add(32);
        }
        if let PaletteAction::Launch(app_id) = action {
            if let Some(index) = windows::app_slot_index(apps, app_id) {
                score = score.saturating_add(apps[index].launch_count.saturating_mul(2));
                if focused_app == Some(app_id) {
                    score = score.saturating_add(24);
                }
                if apps[index].running {
                    score = score.saturating_add(8);
                }
            }
            if let Some(position) = recent_focus[..recent_focus_len.min(recent_focus.len())]
                .iter()
                .position(|candidate| *candidate == app_id)
            {
                score = score.saturating_add((APP_COUNT - position) as u32 * 6);
            }
        }
        ranked[ranked_len] = (action, score);
        ranked_len += 1;
    }

    let mut index = 1usize;
    while index < ranked_len {
        let current = ranked[index];
        let mut scan = index;
        while scan > 0 && ranked[scan - 1].1 < current.1 {
            ranked[scan] = ranked[scan - 1];
            scan -= 1;
        }
        ranked[scan] = current;
        index += 1;
    }

    let count = ranked_len.min(OVERLAY_RESULT_MAX);
    for index in 0..count {
        results[index] = ranked[index].0;
    }
    count
}

pub(crate) fn palette_action_label(action: PaletteAction) -> &'static str {
    match action {
        PaletteAction::Launch(DesktopAppId::Settings) => "Open Settings",
        PaletteAction::Launch(DesktopAppId::Files) => "Open Files",
        PaletteAction::Launch(DesktopAppId::Monitor) => "Open Monitor",
        PaletteAction::Launch(DesktopAppId::Terminal) => "Open Terminal",
        PaletteAction::Launch(DesktopAppId::SoftwareCenter) => "Open Software Center",
        PaletteAction::ShowNotifications => "Show Notification History",
        PaletteAction::ShowClipboardHistory => "Show Clipboard History",
        PaletteAction::ShowMedia => "Show Media and Volume",
        PaletteAction::ToggleNotifications => "Toggle Notification History",
        PaletteAction::ToggleClipboardHistory => "Toggle Clipboard History",
        PaletteAction::ToggleMedia => "Toggle Media and Volume",
        PaletteAction::DismissAllNotifications => "Dismiss All Notifications",
        PaletteAction::CycleSettingsPage => "Cycle Settings Page",
        PaletteAction::LockSession => "Lock Session",
        PaletteAction::SwitchWorkspace(1) => "Switch to Workspace 1",
        PaletteAction::SwitchWorkspace(2) => "Switch to Workspace 2",
        PaletteAction::SwitchWorkspace(3) => "Switch to Workspace 3",
        PaletteAction::SwitchWorkspace(4) => "Switch to Workspace 4",
        PaletteAction::FocusNext => "Focus Next Window",
        PaletteAction::SwitchWorkspace(_) => "Switch Workspace",
    }
}

fn contains_case_fold(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let haystack = haystack.as_bytes();
    let needle = needle.as_bytes();
    if needle.len() > haystack.len() {
        return false;
    }
    for start in 0..=haystack.len() - needle.len() {
        if haystack[start..start + needle.len()]
            .iter()
            .zip(needle.iter())
            .all(|(left, right)| left.to_ascii_lowercase() == right.to_ascii_lowercase())
        {
            return true;
        }
    }
    false
}

fn starts_with_case_fold(haystack: &str, needle: &str) -> bool {
    let haystack = haystack.as_bytes();
    let needle = needle.as_bytes();
    if needle.len() > haystack.len() {
        return false;
    }
    haystack[..needle.len()]
        .iter()
        .zip(needle.iter())
        .all(|(left, right)| left.to_ascii_lowercase() == right.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serviceos_userspace_runtime::ServiceImageId;

    fn slots() -> [AppSlot; 5] {
        [
            AppSlot::new(DesktopAppId::Settings, ServiceImageId::SettingsApp),
            AppSlot::new(DesktopAppId::Files, ServiceImageId::FilesApp),
            AppSlot::new(DesktopAppId::Monitor, ServiceImageId::MonitorApp),
            AppSlot::new(DesktopAppId::Terminal, ServiceImageId::TerminalApp),
            AppSlot::new(
                DesktopAppId::SoftwareCenter,
                ServiceImageId::SoftwareCenterApp,
            ),
        ]
    }

    fn rank(
        apps: &[AppSlot; APP_COUNT],
        focused: Option<DesktopAppId>,
        recent: &[DesktopAppId],
        recent_len: usize,
        query: &str,
    ) -> ([PaletteAction; OVERLAY_RESULT_MAX], usize) {
        let mut results = [PaletteAction::ShowNotifications; OVERLAY_RESULT_MAX];
        let count = rank_palette(apps, focused, recent, recent_len, query, &mut results);
        (results, count)
    }

    #[test]
    fn empty_query_merges_apps_and_system_actions() {
        let mut apps = slots();
        let terminal = windows::app_slot_index(&apps, DesktopAppId::Terminal).unwrap();
        apps[terminal].running = true;
        let (results, count) = rank(&apps, Some(DesktopAppId::Terminal), &[], 0, "");
        assert_eq!(count, OVERLAY_RESULT_MAX);
        assert_eq!(results[0], PaletteAction::Launch(DesktopAppId::Terminal));
        let launches = results[..count].iter().filter(|action| matches!(action, PaletteAction::Launch(_))).count();
        let system = count - launches;
        assert!(launches >= 1);
        assert!(system >= 1, "system actions must rank alongside apps");
    }

    #[test]
    fn global_system_actions_reachable_by_query() {
        for (query, expected) in [
            ("media", PaletteAction::ToggleMedia),
            ("lock", PaletteAction::LockSession),
            ("settings", PaletteAction::CycleSettingsPage),
            ("dismiss", PaletteAction::DismissAllNotifications),
            ("workspace", PaletteAction::SwitchWorkspace(4)),
        ] {
            let (results, count) = rank(&slots(), None, &[], 0, query);
            assert!(count != 0, "query {query} matched nothing");
            assert!(
                results[..count].contains(&expected),
                "query {query} missing {:?}",
                expected
            );
        }
    }

    #[test]
    fn running_recent_app_outranks_static_action_on_prefix_query() {
        let mut apps = slots();
        let terminal = windows::app_slot_index(&apps, DesktopAppId::Terminal).unwrap();
        apps[terminal].running = true;
        let recent = [DesktopAppId::Terminal; 5];
        let (results, count) = rank(&apps, Some(DesktopAppId::Terminal), &recent, 1, "te");
        assert!(count >= 2);
        assert_eq!(results[0], PaletteAction::Launch(DesktopAppId::Terminal));
    }

    #[test]
    fn launch_count_breaks_ties_between_apps() {
        let mut apps = slots();
        let files = windows::app_slot_index(&apps, DesktopAppId::Files).unwrap();
        let monitor = windows::app_slot_index(&apps, DesktopAppId::Monitor).unwrap();
        apps[files].launch_count = 5;
        apps[monitor].launch_count = 0;
        let (results, _) = rank(&apps, None, &[], 0, "open ");
        let files_rank = results[..]
            .iter()
            .position(|action| *action == PaletteAction::Launch(DesktopAppId::Files))
            .unwrap();
        let monitor_rank = results[..]
            .iter()
            .position(|action| *action == PaletteAction::Launch(DesktopAppId::Monitor))
            .unwrap_or(usize::MAX);
        assert!(files_rank < monitor_rank);
    }

    #[test]
    fn clipboard_query_ranks_history_actions_and_filters_rest() {
        let (results, count) = rank(&slots(), None, &[], 0, "clipboard");
        assert!(count >= 2);
        assert_eq!(results[0], PaletteAction::ShowClipboardHistory);
        assert_eq!(results[1], PaletteAction::ToggleClipboardHistory);
        for action in results[..count].iter() {
            let label = palette_action_label(*action).to_ascii_lowercase();
            assert!(label.contains("clipboard"), "unexpected action {label}");
        }
    }

    #[test]
    fn no_match_yields_zero_results() {
        let (_, count) = rank(&slots(), None, &[], 0, "zzzz");
        assert_eq!(count, 0);
    }
}
