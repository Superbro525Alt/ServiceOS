use serviceos_userspace_runtime::DesktopAppId;

use crate::{
    state::{APP_COUNT, DesktopState, OVERLAY_RESULT_MAX, PaletteAction},
    windows,
};

pub(crate) fn palette_matches(
    state: &DesktopState,
    results: &mut [PaletteAction; OVERLAY_RESULT_MAX],
) -> usize {
    let query = core::str::from_utf8(&state.palette_query[..state.palette_query_len]).unwrap_or("");
    let actions = [
        PaletteAction::Launch(DesktopAppId::Settings),
        PaletteAction::Launch(DesktopAppId::Files),
        PaletteAction::Launch(DesktopAppId::Monitor),
        PaletteAction::Launch(DesktopAppId::Terminal),
        PaletteAction::Launch(DesktopAppId::SoftwareCenter),
        PaletteAction::ShowNotifications,
        PaletteAction::ShowClipboardHistory,
        PaletteAction::SwitchWorkspace(1),
        PaletteAction::SwitchWorkspace(2),
        PaletteAction::SwitchWorkspace(3),
        PaletteAction::SwitchWorkspace(4),
        PaletteAction::FocusNext,
    ];
    let mut ranked = [(PaletteAction::ShowNotifications, 0u32); 12];
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
            if let Some(index) = windows::app_slot_index(&state.apps, app_id) {
                score = score.saturating_add(state.apps[index].launch_count.saturating_mul(2));
                if state.focused_app == Some(app_id) {
                    score = score.saturating_add(24);
                }
                if state.apps[index].running {
                    score = score.saturating_add(8);
                }
            }
            if let Some(position) = state.recent_focus[..state.recent_focus_len]
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
