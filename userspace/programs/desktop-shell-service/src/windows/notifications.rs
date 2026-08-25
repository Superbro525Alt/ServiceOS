use super::*;

pub(crate) fn mru_promote(
    recent: &mut [DesktopAppId],
    len: &mut usize,
    app_id: DesktopAppId,
) {
    if let Some(index) = recent[..*len].iter().position(|candidate| *candidate == app_id) {
        for scan in (1..=index).rev() {
            recent[scan] = recent[scan - 1];
        }
        recent[0] = app_id;
        return;
    }

    let capacity = recent.len();
    let limit = (*len).min(capacity - 1);
    for index in (0..limit).rev() {
        recent[index + 1] = recent[index];
    }
    recent[0] = app_id;
    if *len < capacity {
        *len += 1;
    }
}

pub(crate) fn push_recent_focus(state: &mut DesktopState, app_id: DesktopAppId) {
    mru_promote(&mut state.recent_focus, &mut state.recent_focus_len, app_id);
}

pub(crate) fn dismiss_all_notifications(state: &mut DesktopState) {
    for index in 0..state.notification_history_len {
        state.notification_history[index] = NotificationEntry::empty();
    }
    state.notification_history_len = 0;
    state.overlay_selection = 0;
}

pub(crate) fn post_notification(
    state: &mut DesktopState,
    source_app: Option<DesktopAppId>,
    actionable: bool,
    text: &[u8],
) -> rt::Result<()> {
    let live_len = text.len().min(MAX_NOTIFICATION_BYTES);
    state.notification[..live_len].copy_from_slice(&text[..live_len]);
    state.notification_len = live_len;
    state.notification_deadline = rt::monotonic_now()?.saturating_add(NOTIFICATION_TIMEOUT_TICKS);

    let history_len = text.len().min(NOTIFICATION_HISTORY_TEXT_MAX);
    let limit = state
        .notification_history_len
        .min(NOTIFICATION_HISTORY_MAX - 1);
    for index in (0..limit).rev() {
        state.notification_history[index + 1] = state.notification_history[index];
    }
    state.notification_history[0] = NotificationEntry::empty();
    state.notification_history[0].occupied = true;
    state.notification_history[0].sequence = state.next_notification_sequence;
    state.notification_history[0].source_app = source_app;
    state.notification_history[0].actionable = actionable;
    state.notification_history[0].text_len = history_len;
    state.notification_history[0].text[..history_len].copy_from_slice(&text[..history_len]);
    state.next_notification_sequence = state.next_notification_sequence.saturating_add(1);
    if state.notification_history_len < NOTIFICATION_HISTORY_MAX {
        state.notification_history_len += 1;
    }
    if state.overlay_mode == OverlayMode::Notifications {
        state.overlay_selection = state
            .overlay_selection
            .min(state.notification_history_len.saturating_sub(1));
    }
    render_desktop(state)
}
