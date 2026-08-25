use rt::ServiceId;
use serviceos_userspace_runtime as rt;

use crate::catalog_meta::{self, MAX_QUERY_BYTES};

pub(crate) const MAX_ENTRIES: usize = 24;
pub(crate) const MAX_CATEGORY_BYTES: usize = 24;
pub(crate) const MAX_SUMMARY_BYTES: usize = 72;
pub(crate) const MAX_STATUS_BYTES: usize = 80;
pub(crate) const MAX_SOURCE_BYTES: usize = 96;
pub(crate) const BUFFER_WIDTH: u32 = 1024;
pub(crate) const BUFFER_HEIGHT: u32 = 768;
pub(crate) const BUFFER_BYTES: usize = BUFFER_WIDTH as usize * BUFFER_HEIGHT as usize * 4;
pub(crate) const SURFACE_BUFFER_SLOTS: usize = 2;
pub(crate) const PIXEL_STRIDE: usize = BUFFER_WIDTH as usize;
pub(crate) const OUTER_PAD: i32 = 14;
pub(crate) const CONTENT_GAP: i32 = 12;
pub(crate) const HEADER_HEIGHT: i32 = 56;
pub(crate) const PANEL_TITLE_HEIGHT: i32 = 26;
pub(crate) const ROW_HEIGHT: i32 = 30;
pub(crate) const BUTTON_HEIGHT: i32 = 22;
pub(crate) const ACTION_BUTTON_WIDTH: i32 = 104;
pub(crate) const ACTION_BUTTON_GAP: i32 = 18;
pub(crate) const STATUS_BAR_HEIGHT: i32 = 24;
pub(crate) const KEY_ENTER: u32 = 28;
pub(crate) const KEY_BACKSPACE: u32 = 14;
pub(crate) const KEY_DELETE: u32 = 111;
pub(crate) const KEY_ESC: u32 = 1;
pub(crate) const KEY_R: u32 = 19;
pub(crate) const KEY_L: u32 = 38;
pub(crate) const KEY_TAB: u32 = 15;
pub(crate) const KEY_UP: u32 = 103;
pub(crate) const KEY_PAGE_UP: u32 = 104;
pub(crate) const KEY_DOWN: u32 = 108;
pub(crate) const KEY_PAGE_DOWN: u32 = 109;

#[derive(Clone, Copy)]
pub(crate) struct Layout {
    pub(crate) header_x: i32,
    pub(crate) header_y: i32,
    pub(crate) header_w: i32,
    pub(crate) left_x: i32,
    pub(crate) left_y: i32,
    pub(crate) left_w: i32,
    pub(crate) left_h: i32,
    pub(crate) right_x: i32,
    pub(crate) right_y: i32,
    pub(crate) right_w: i32,
    pub(crate) right_h: i32,
    pub(crate) list_rows_y: i32,
    pub(crate) list_rows_h: i32,
    pub(crate) sync_x0: i32,
    pub(crate) sync_x1: i32,
    pub(crate) sync_y0: i32,
    pub(crate) sync_y1: i32,
    pub(crate) install_x0: i32,
    pub(crate) install_x1: i32,
    pub(crate) install_y0: i32,
    pub(crate) install_y1: i32,
    pub(crate) remove_x0: i32,
    pub(crate) remove_x1: i32,
    pub(crate) remove_y0: i32,
    pub(crate) remove_y1: i32,
    pub(crate) detail_title_y: i32,
    pub(crate) detail_body_y: i32,
    pub(crate) detail_chip_y: i32,
    pub(crate) detail_text_w: i32,
    pub(crate) action_badge_y: i32,
    pub(crate) status_y: i32,
}

impl Layout {
    pub(crate) fn visible_rows(self) -> usize {
        self.list_rows_h.max(0) as usize / ROW_HEIGHT as usize
    }
}

#[derive(Clone, Copy)]
pub(crate) struct CatalogEntry {
    pub(crate) service_id: ServiceId,
    pub(crate) repo_index: u32,
    pub(crate) installed: bool,
    pub(crate) active: bool,
    pub(crate) rollback: bool,
    pub(crate) latest_version: [u8; 24],
    pub(crate) latest_version_len: usize,
    pub(crate) category: [u8; MAX_CATEGORY_BYTES],
    pub(crate) category_len: usize,
    pub(crate) summary: [u8; MAX_SUMMARY_BYTES],
    pub(crate) summary_len: usize,
}

impl CatalogEntry {
    pub(crate) const fn empty() -> Self {
        Self {
            service_id: ServiceId::RootManager,
            repo_index: 0,
            installed: false,
            active: false,
            rollback: false,
            latest_version: [0; 24],
            latest_version_len: 0,
            category: [0; MAX_CATEGORY_BYTES],
            category_len: 0,
            summary: [0; MAX_SUMMARY_BYTES],
            summary_len: 0,
        }
    }
}

pub(crate) struct AppState {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) focused: bool,
    pub(crate) entries: [CatalogEntry; MAX_ENTRIES],
    pub(crate) entry_count: usize,
    pub(crate) selected_index: usize,
    pub(crate) scroll_offset: usize,
    pub(crate) query: [u8; MAX_QUERY_BYTES],
    pub(crate) query_len: usize,
    pub(crate) category_filter: usize,
    /// Indices into `entries` for the visible (filtered + ranked) view.
    pub(crate) view: [usize; MAX_ENTRIES],
    pub(crate) view_count: usize,
    pub(crate) status: [u8; MAX_STATUS_BYTES],
    pub(crate) status_len: usize,
    /// Updates performed through this app this session: (service, tick).
    /// The package contract carries no timestamp, so last-update time is
    /// shown only where actually observed.
    pub(crate) session_updates: [(ServiceId, u64); MAX_ENTRIES],
    pub(crate) session_update_count: usize,
}

pub(crate) const CATEGORY_FILTERS: [&str; 5] =
    ["All", "Messaging", "Runtime", "Development", "System"];

impl AppState {
    pub(crate) fn new(width: u32, height: u32, focused: bool) -> Self {
        Self {
            width,
            height,
            focused,
            entries: [CatalogEntry::empty(); MAX_ENTRIES],
            entry_count: 0,
            selected_index: 0,
            scroll_offset: 0,
            query: [0; MAX_QUERY_BYTES],
            query_len: 0,
            category_filter: 0,
            view: [0; MAX_ENTRIES],
            view_count: 0,
            status: [0; MAX_STATUS_BYTES],
            status_len: 0,
            session_updates: [(ServiceId::RootManager, 0); MAX_ENTRIES],
            session_update_count: 0,
        }
    }

    /// Record an update performed through this app (bounded ring).
    pub(crate) fn record_session_update(&mut self, service_id: ServiceId, tick: u64) {
        let slot = self
            .session_updates
            .iter_mut()
            .take(self.session_update_count)
            .find(|(recorded, _)| *recorded == service_id);
        match slot {
            Some((_, recorded_tick)) => *recorded_tick = tick,
            None => {
                if self.session_update_count < MAX_ENTRIES {
                    self.session_updates[self.session_update_count] = (service_id, tick);
                    self.session_update_count += 1;
                } else {
                    // Ring full: drop the oldest entry by shifting down.
                    self.session_updates.rotate_left(1);
                    self.session_updates[MAX_ENTRIES - 1] = (service_id, tick);
                }
            }
        }
    }

    pub(crate) fn session_update_tick(&self, service_id: ServiceId) -> Option<u64> {
        self.session_updates
            .iter()
            .take(self.session_update_count)
            .find(|(recorded, _)| *recorded == service_id)
            .map(|(_, tick)| *tick)
    }
}

pub(crate) fn rebuild_view(state: &mut AppState) {
    let count = state.entry_count;
    let filter = CATEGORY_FILTERS
        .get(state.category_filter)
        .copied()
        .unwrap_or("All");
    let mut view = [0usize; MAX_ENTRIES];
    let mut view_count = 0usize;

    if filter != "All" || state.query_len > 0 {
        // Filtered path: category gate first, then search ranking when a
        // query is active. Unranked matches keep catalog order.
        let mut candidates = [0usize; MAX_ENTRIES];
        let mut candidate_count = 0usize;
        for index in 0..count {
            let doc = catalog_meta::doc_for(
                state.entries[index].service_id,
                &state.entries[index].category[..state.entries[index].category_len],
            );
            if filter != "All" && !catalog_meta::field_eq_ci(doc.category, filter) {
                continue;
            }
            candidates[candidate_count] = index;
            candidate_count += 1;
        }

        if state.query_len > 0 {
            let query_bytes = &state.query[..state.query_len];
            if let Ok(query) = core::str::from_utf8(query_bytes) {
                let mut ranked = [0usize; MAX_ENTRIES];
                let hit_count = catalog_meta::rank_docs(
                    candidate_count,
                    |position| {
                        let entry = &state.entries[candidates[position]];
                        catalog_meta::doc_for(
                            entry.service_id,
                            &entry.category[..entry.category_len],
                        )
                    },
                    query,
                    &mut ranked,
                );
                for position in 0..hit_count {
                    view[view_count] = candidates[ranked[position]];
                    view_count += 1;
                }
                state.view = view;
                state.view_count = view_count;
                return;
            }
        }

        for &candidate in candidates.iter().take(candidate_count) {
            view[view_count] = candidate;
            view_count += 1;
        }
    } else {
        for (index, slot) in view.iter_mut().enumerate().take(count) {
            *slot = index;
        }
        view_count = count;
    }

    state.view = view;
    state.view_count = view_count;
}

pub(crate) fn selected_entry(state: &AppState) -> Option<CatalogEntry> {
    state
        .view
        .get(state.selected_index)
        .copied()
        .filter(|index| *index < state.entry_count)
        .map(|index| state.entries[index])
}

pub(crate) fn installed_count(state: &AppState) -> usize {
    state.entries[..state.entry_count]
        .iter()
        .filter(|entry| entry.installed)
        .count()
}

pub(crate) fn select_service(state: &mut AppState, service_id: ServiceId) {
    if let Some(position) = state.view[..state.view_count]
        .iter()
        .position(|index| state.entries[*index].service_id == service_id)
    {
        state.selected_index = position;
        ensure_selected_visible(state);
    }
}

pub(crate) fn visible_row_count(height: u32) -> usize {
    compute_layout_for_height(height).visible_rows()
}

pub(crate) fn ensure_selected_visible(state: &mut AppState) {
    let visible = visible_row_count(state.height).max(1);
    if state.selected_index < state.scroll_offset {
        state.scroll_offset = state.selected_index;
    } else if state.selected_index >= state.scroll_offset + visible {
        state.scroll_offset = state.selected_index + 1 - visible;
    }
}

pub(crate) fn clamp_view(state: &mut AppState) {
    let visible = visible_row_count(state.height).max(1);
    let max_scroll = state.view_count.saturating_sub(visible);
    if state.scroll_offset > max_scroll {
        state.scroll_offset = max_scroll;
    }
    if state.selected_index >= state.view_count && state.view_count != 0 {
        state.selected_index = state.view_count - 1;
    }
}

pub(crate) fn scroll_up(state: &mut AppState, amount: usize) {
    state.scroll_offset = state.scroll_offset.saturating_sub(amount.max(1));
    if state.selected_index < state.scroll_offset {
        state.selected_index = state.scroll_offset;
    }
}

pub(crate) fn scroll_down(state: &mut AppState, amount: usize) {
    let visible = visible_row_count(state.height).max(1);
    let max_scroll = state.view_count.saturating_sub(visible);
    state.scroll_offset = (state.scroll_offset + amount.max(1)).min(max_scroll);
    if state.selected_index < state.scroll_offset {
        state.selected_index = state.scroll_offset;
    }
}

fn reset_selection(state: &mut AppState) {
    rebuild_view(state);
    if state.view_count == 0 {
        state.selected_index = 0;
        state.scroll_offset = 0;
    } else {
        if state.selected_index >= state.view_count {
            state.selected_index = state.view_count - 1;
        }
        ensure_selected_visible(state);
    }
}

pub(crate) fn push_query_char(state: &mut AppState, byte: u8) -> bool {
    if state.query_len >= state.query.len() {
        return false;
    }
    state.query[state.query_len] = byte;
    state.query_len += 1;
    state.selected_index = 0;
    state.scroll_offset = 0;
    reset_selection(state);
    true
}

pub(crate) fn pop_query_char(state: &mut AppState) -> bool {
    if state.query_len == 0 {
        return false;
    }
    state.query_len -= 1;
    state.query[state.query_len] = 0;
    state.selected_index = 0;
    state.scroll_offset = 0;
    reset_selection(state);
    true
}

pub(crate) fn clear_query(state: &mut AppState) -> bool {
    if state.query_len == 0 {
        return false;
    }
    while state.query_len > 0 {
        state.query_len -= 1;
        state.query[state.query_len] = 0;
    }
    reset_selection(state);
    true
}

pub(crate) fn cycle_category_filter(state: &mut AppState) -> bool {
    state.category_filter = (state.category_filter + 1) % CATEGORY_FILTERS.len();
    state.selected_index = 0;
    state.scroll_offset = 0;
    reset_selection(state);
    true
}

pub(crate) fn query_text(state: &AppState) -> &str {
    core::str::from_utf8(&state.query[..state.query_len]).unwrap_or("")
}

pub(crate) fn compute_layout(state: &AppState) -> Layout {
    compute_layout_for_dims(
        state.width.min(BUFFER_WIDTH) as i32,
        state.height.min(BUFFER_HEIGHT) as i32,
        selected_entry(state)
            .map(|entry| service_title(entry.service_id))
            .unwrap_or("Select a package"),
    )
}

fn compute_layout_for_height(height: u32) -> Layout {
    compute_layout_for_dims(
        BUFFER_WIDTH as i32,
        height.min(BUFFER_HEIGHT) as i32,
        "Select a package",
    )
}

fn compute_layout_for_dims(width: i32, height: i32, selected_title: &str) -> Layout {
    let content_top = serviceos_desktop_ui::TITLEBAR_HEIGHT as i32 + OUTER_PAD;
    let header_x = OUTER_PAD;
    let header_y = content_top;
    let header_w = width - OUTER_PAD * 2;
    let body_y = header_y + HEADER_HEIGHT + CONTENT_GAP;
    let body_h = height - body_y - OUTER_PAD;
    let mut left_w = ((header_w - CONTENT_GAP) * 38) / 100;
    left_w = left_w.clamp(300, 388.min(header_w - 220));
    let right_w = header_w - CONTENT_GAP - left_w;
    let left_x = OUTER_PAD;
    let left_y = body_y;
    let right_x = left_x + left_w + CONTENT_GAP;
    let right_y = body_y;
    let detail_title_y = right_y + 40;
    let install_x0 = right_x + right_w - ACTION_BUTTON_WIDTH - 12;
    let install_x1 = install_x0 + ACTION_BUTTON_WIDTH;
    let remove_x0 = install_x0;
    let remove_x1 = install_x1;
    let install_y0 = detail_title_y - 2;
    let install_y1 = install_y0 + BUTTON_HEIGHT;
    let remove_y0 = install_y1 + ACTION_BUTTON_GAP;
    let remove_y1 = remove_y0 + BUTTON_HEIGHT;
    let detail_text_w = (install_x0 - (right_x + 12) - 12).max(64);
    let detail_chip_y = detail_title_y + 34;
    let detail_body_y = detail_chip_y + 26;
    let action_badge_y = remove_y1 + 14;
    let sync_y0 = header_y + 18;
    let sync_y1 = sync_y0 + BUTTON_HEIGHT;
    let sync_x1 = header_x + header_w - 14;
    let sync_x0 = sync_x1 - 88;
    let list_rows_y = left_y + PANEL_TITLE_HEIGHT + 8;
    let status_y = right_y + body_h - STATUS_BAR_HEIGHT - 12;
    let _ = selected_title;
    Layout {
        header_x,
        header_y,
        header_w,
        left_x,
        left_y,
        left_w,
        left_h: body_h,
        right_x,
        right_y,
        right_w,
        right_h: body_h,
        list_rows_y,
        list_rows_h: body_h - PANEL_TITLE_HEIGHT - 16,
        sync_x0,
        sync_x1,
        sync_y0,
        sync_y1,
        install_x0,
        install_x1,
        install_y0,
        install_y1,
        remove_x0,
        remove_x1,
        remove_y0,
        remove_y1,
        detail_title_y,
        detail_body_y,
        detail_chip_y,
        detail_text_w,
        action_badge_y,
        status_y,
    }
}

pub(crate) fn service_title(service_id: ServiceId) -> &'static str {
    match service_id {
        ServiceId::Announce => "Announce",
        ServiceId::Runtime => "Runtime Tools",
        ServiceId::Developer => "Developer SDK",
        _ => service_label(service_id),
    }
}

pub(crate) fn service_label(service_id: ServiceId) -> &'static str {
    match service_id {
        ServiceId::RootManager => "root-manager",
        ServiceId::Storage => "storage-service",
        ServiceId::Console => "console-service",
        ServiceId::Config => "config-service",
        ServiceId::Log => "log-service",
        ServiceId::Status => "status-service",
        ServiceId::Shell => "shell-service",
        ServiceId::Package => "package-service",
        ServiceId::Announce => "announce-service",
        ServiceId::Network => "network-service",
        ServiceId::Graphics => "graphics-service",
        ServiceId::Session => "session-service",
        ServiceId::DesktopShell => "desktop-shell-service",
        ServiceId::Terminal => "terminal-service",
        ServiceId::Audio => "audio-service",
        ServiceId::Runtime => "runtime-service",
        ServiceId::Developer => "developer-service",
        ServiceId::Clipboard => "clipboard-service",
        ServiceId::Security => "security-service",
        ServiceId::SetupWizard => "setup-wizard",
        ServiceId::Backup => "backup-service",
    }
}
