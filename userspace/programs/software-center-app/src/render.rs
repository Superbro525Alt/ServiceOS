use core::{fmt::Write, str};

use rt::FixedLogBuffer;
use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;

use crate::actions::{action_label, channel_label, ring_label, text_or_dash, trust_badge};
use crate::catalog_meta::{self, MAX_QUERY_BYTES};
use crate::repositories::{self, AddField, LEDGER_NOTE, SIDELOAD_NOTE};
use crate::state::{
    AppState, BUFFER_BYTES, BUFFER_HEIGHT, BUFFER_WIDTH, CATEGORY_FILTERS, CatalogEntry,
    HEADER_HEIGHT, Layout, MAX_SOURCE_BYTES, PIXEL_STRIDE, ROW_HEIGHT, STATUS_BAR_HEIGHT,
    compute_layout, installed_count, query_text, selected_entry, service_title,
};

pub(crate) fn render(
    presenter: &mut ui::FirstPresentSurface,
    buffer_slot: u32,
    buffer: &mut rt::MappedMemory,
    package_handle: rt::Handle,
    state: &AppState,
) -> rt::Result<()> {
    let layout = compute_layout(state);
    let width = state.width.min(BUFFER_WIDTH) as usize;
    let height = state.height.min(BUFFER_HEIGHT) as usize;
    let bytes = &mut buffer.as_slice_mut()[..BUFFER_BYTES];
    if state.sources.open {
        return draw_sources(presenter, buffer_slot, bytes, layout, state);
    }
    let mut detail0 = FixedLogBuffer::<64>::new();
    let mut detail1 = FixedLogBuffer::<80>::new();
    let mut detail2 = FixedLogBuffer::<80>::new();
    let mut detail3 = FixedLogBuffer::<96>::new();
    let mut unsupported_chip: Option<&'static str> = None;
    let mut recommendations =
        [catalog_meta::Recommendation::EMPTY; catalog_meta::MAX_RECOMMENDATIONS];
    let recommendation_count = {
        let inputs = |index: usize| {
            let entry = &state.entries[index];
            let doc = catalog_meta::doc_for(
                entry.service_id,
                &entry.category[..entry.category_len],
            );
            catalog_meta::RecommendInput {
                category: doc.category,
                keywords: doc.keywords,
                installed: entry.installed,
            }
        };
        catalog_meta::rank_recommendations(state.entry_count, inputs, &mut recommendations)
    };
    if let Some(entry) = selected_entry(state) {
        let mut installed = [0u8; 24];
        let mut active = [0u8; 24];
        let mut rollback = [0u8; 24];
        let mut latest = [0u8; 24];
        let mut source = [0u8; MAX_SOURCE_BYTES];
        if let Ok(provenance) = rt::package_provenance(
            package_handle,
            entry.service_id,
            &mut installed,
            &mut active,
            &mut rollback,
            &mut latest,
            &mut source,
        ) {
            let _ = write!(&mut detail0, "{}", service_title(entry.service_id));
            let description = catalog_meta::description_for(entry.service_id);
            if description.is_empty() {
                let _ = write!(
                    &mut detail1,
                    "{}  repo={}  {}",
                    text_or_dash(&entry.summary[..entry.summary_len]),
                    provenance.repo_index,
                    trust_badge(provenance.trust_state),
                );
            } else {
                let _ = write!(&mut detail1, "{}", description);
            }
            // Update/remove visibility: flag older installs against the
            // newest catalog version and note in-session update times.
            if entry.installed {
                let decision = catalog_meta::decide_update(
                    core::str::from_utf8(&installed[..provenance.installed_version_len]).ok(),
                    core::str::from_utf8(&latest[..provenance.latest_version_len]).ok(),
                );
                let _ = write!(
                    &mut detail2,
                    "latest={}  installed={}  active={}  {}",
                    text_or_dash(&latest[..provenance.latest_version_len]),
                    text_or_dash(&installed[..provenance.installed_version_len]),
                    text_or_dash(&active[..provenance.active_version_len]),
                    decision.label(),
                );
                if let Some(tick) = state.session_update_tick(entry.service_id) {
                    let _ = write!(&mut detail2, "  here@tick{}", tick);
                }
            } else {
                let _ = write!(
                    &mut detail2,
                    "latest={}  installed={}  active={}",
                    text_or_dash(&latest[..provenance.latest_version_len]),
                    text_or_dash(&installed[..provenance.installed_version_len]),
                    text_or_dash(&active[..provenance.active_version_len]),
                );
            }
            if entry.installed {
                let _ = write!(
                    &mut detail3,
                    "channel={}  ring={}  rollback={}  launch=L (run pkg {})",
                    channel_label(provenance.channel),
                    ring_label(provenance.ring),
                    text_or_dash(&rollback[..provenance.rollback_version_len]),
                    crate::state::service_label(entry.service_id),
                );
            } else {
                let _ = write!(
                    &mut detail3,
                    "channel={}  ring={}  rollback={}",
                    channel_label(provenance.channel),
                    ring_label(provenance.ring),
                    text_or_dash(&rollback[..provenance.rollback_version_len]),
                );
            }
            let _ = source;
        } else {
            let _ = write!(&mut detail0, "{}", service_title(entry.service_id));
            let description = catalog_meta::description_for(entry.service_id);
            if description.is_empty() {
                let _ = write!(
                    &mut detail1,
                    "{}",
                    text_or_dash(&entry.summary[..entry.summary_len])
                );
            } else {
                let _ = write!(&mut detail1, "{}", description);
            }
            let _ = write!(
                &mut detail2,
                "latest={}",
                text_or_dash(&entry.latest_version[..entry.latest_version_len])
            );
            let _ = write!(
                &mut detail3,
                "category={}",
                catalog_meta::category_for(entry.service_id, &entry.category[..entry.category_len])
            );
        }
        let targets = catalog_meta::targets_for(entry.service_id);
        let _ = write!(
            &mut detail3,
            "  host={}",
            catalog_meta::compat_label(targets)
        );
        if !catalog_meta::host_supported(targets) {
            unsupported_chip = Some("UNSUPPORTED ON THIS HOST");
        }
        let screenshot_ref = catalog_meta::screenshot_ref_for(entry.service_id);
        if !screenshot_ref.is_empty() {
            let _ = write!(&mut detail3, "  shot={}", screenshot_ref);
        }
        if let Some(tenths) = catalog_meta::rating_tenths_for(entry.service_id) {
            let bar = catalog_meta::star_bar(tenths);
            let _ = write!(
                &mut detail3,
                "  rating={}.{} {}",
                tenths / 10,
                tenths % 10,
                str::from_utf8(&bar).unwrap_or("-----")
            );
        }
    } else {
        let _ = write!(&mut detail0, "Select a package");
        let _ = write!(
            &mut detail1,
            "Browse the catalog and inspect trust before installing."
        );
    }

    ui::draw_window_frame_rgba8888(
        bytes,
        PIXEL_STRIDE,
        width,
        height,
        state.focused,
        ui::BG_WINDOW_ALT,
        "SOFTWARE CENTER",
    );
    draw_header(bytes, layout, state);
    draw_panel(
        bytes,
        layout.left_x,
        layout.left_y,
        layout.left_w,
        layout.left_h,
        ui::BG_PANEL,
    );
    draw_panel(
        bytes,
        layout.right_x,
        layout.right_y,
        layout.right_w,
        layout.right_h,
        ui::BG_PANEL,
    );
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        layout.left_x + 12,
        layout.left_y + 10,
        ui::TEXT_PRIMARY,
        "CATALOG",
    );
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        layout.right_x + 12,
        layout.right_y + 10,
        ui::TEXT_PRIMARY,
        "DETAILS",
    );
    draw_button(
        bytes,
        layout.sync_x0,
        layout.sync_y0,
        layout.sync_x1,
        layout.sync_y1,
        ui::ACCENT_DIM,
        "SYNC ALL",
        ui::TEXT_PRIMARY,
    );
    draw_details(
        bytes,
        layout,
        str::from_utf8(detail0.as_bytes()).unwrap_or("PACKAGE"),
        str::from_utf8(detail1.as_bytes()).unwrap_or("DETAILS"),
        str::from_utf8(detail2.as_bytes()).unwrap_or("DETAILS"),
        str::from_utf8(detail3.as_bytes()).unwrap_or("DETAILS"),
        str::from_utf8(&state.status[..state.status_len]).unwrap_or(""),
        selected_entry(state),
        unsupported_chip,
    );
    let mut section_y = layout.detail_body_y + 44;
    if let Some(entry) = selected_entry(state) {
        draw_screenshot_card(bytes, layout, section_y, entry.service_id);
        section_y += SCREENSHOT_CARD_HEIGHT + 16;
    }
    draw_recommendations(bytes, layout, section_y, &recommendations[..recommendation_count], state);
    draw_button(
        bytes,
        layout.install_x0,
        layout.install_y0,
        layout.install_x1,
        layout.install_y1,
        if selected_entry(state).is_some_and(|entry| entry.installed) {
            ui::STATUS_OK
        } else {
            ui::ACCENT
        },
        action_label(selected_entry(state)),
        ui::BG_PANEL,
    );
    draw_button(
        bytes,
        layout.remove_x0,
        layout.remove_y0,
        layout.remove_x1,
        layout.remove_y1,
        ui::STATUS_WARN,
        "REMOVE",
        ui::BG_PANEL,
    );
    draw_list(bytes, layout, state);
    presenter.present(
        buffer_slot,
        state.width.min(BUFFER_WIDTH),
        state.height.min(BUFFER_HEIGHT),
    )
}

fn draw_header(bytes: &mut [u8], layout: Layout, state: &AppState) {
    draw_panel(
        bytes,
        layout.header_x,
        layout.header_y,
        layout.header_w,
        HEADER_HEIGHT,
        ui::BG_PANEL,
    );
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        layout.header_x + 14,
        layout.header_y + 12,
        ui::TEXT_PRIMARY,
        "DISCOVER AND MANAGE SOFTWARE",
    );
    let mut summary = FixedLogBuffer::<64>::new();
    let _ = write!(
        &mut summary,
        "{} packages  {} installed",
        state.entry_count,
        installed_count(state),
    );
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        layout.header_x + 14,
        layout.header_y + 28,
        ui::TEXT_SECONDARY,
        str::from_utf8(summary.as_bytes()).unwrap_or(""),
    );

    let filter = CATEGORY_FILTERS
        .get(state.category_filter)
        .copied()
        .unwrap_or("All");
    let mut search = FixedLogBuffer::<{ MAX_QUERY_BYTES + 48 }>::new();
    if query_text(state).is_empty() {
        let _ = write!(
            &mut search,
            "find: type to search   cat:{}  tab=next  s=sources",
            filter
        );
    } else {
        let _ = write!(
            &mut search,
            "find:{}  {} hits  cat:{}",
            query_text(state),
            state.view_count,
            filter
        );
    }
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        layout.sync_x0 - (search.as_bytes().len() as i32 + 2) * 8,
        layout.header_y + 12 + 16,
        ui::TEXT_MUTED,
        str::from_utf8(search.as_bytes()).unwrap_or(""),
    );
}

fn draw_details(
    bytes: &mut [u8],
    layout: Layout,
    detail0: &str,
    detail1: &str,
    detail2: &str,
    detail3: &str,
    status: &str,
    entry: Option<CatalogEntry>,
    unsupported_chip: Option<&'static str>,
) {
    let meta_x = layout.right_x + 12;
    let title_y = layout.detail_title_y;
    draw_text_fit(
        bytes,
        meta_x,
        title_y,
        ui::TEXT_PRIMARY,
        detail0,
        layout.detail_text_w,
    );
    draw_text_fit(
        bytes,
        meta_x,
        title_y + 16,
        ui::TEXT_SECONDARY,
        detail1,
        layout.detail_text_w,
    );
    if let Some(entry) = entry {
        draw_chip(
            bytes,
            meta_x,
            layout.detail_chip_y,
            catalog_meta::category_for(entry.service_id, &entry.category[..entry.category_len]),
            ui::ACCENT_DIM,
            ui::TEXT_PRIMARY,
        );
        if let Some(chip) = unsupported_chip {
            draw_chip(
                bytes,
                meta_x + 140,
                layout.detail_chip_y,
                chip,
                ui::STATUS_WARN,
                ui::BG_PANEL,
            );
        }
        if entry.installed {
            draw_chip(
                bytes,
                layout.install_x0,
                layout.action_badge_y,
                "INSTALLED",
                ui::STATUS_OK,
                ui::BG_PANEL,
            );
        }
        if entry.active {
            let active_y = if entry.installed {
                layout.action_badge_y + 20
            } else {
                layout.action_badge_y
            };
            draw_chip(
                bytes,
                layout.install_x0,
                active_y,
                "ACTIVE",
                ui::ACCENT,
                ui::BG_PANEL,
            );
        }
    }
    draw_text_fit(
        bytes,
        meta_x,
        layout.detail_body_y,
        ui::TEXT_SECONDARY,
        detail2,
        layout.detail_text_w,
    );
    draw_text_fit(
        bytes,
        meta_x,
        layout.detail_body_y + 14,
        ui::TEXT_SECONDARY,
        detail3,
        layout.detail_text_w,
    );
    draw_status_bar(
        bytes,
        layout.right_x + 12,
        layout.status_y,
        layout.right_w - 24,
        status,
    );
}

fn draw_list(bytes: &mut [u8], layout: Layout, state: &AppState) {
    let visible_rows = layout.visible_rows();
    for row in 0..visible_rows {
        let view_position = state.scroll_offset + row;
        if view_position >= state.view_count {
            break;
        }
        let entry_index = state.view[view_position];
        if entry_index >= state.entry_count {
            break;
        }
        let entry = state.entries[entry_index];
        let row_y = layout.list_rows_y as usize + row * ROW_HEIGHT as usize;
        let selected = entry_index == state.selected_index;
        ui::fill_rgba8888_rect(
            bytes,
            PIXEL_STRIDE,
            BUFFER_WIDTH as usize,
            BUFFER_HEIGHT as usize,
            (layout.left_x + 8) as usize,
            row_y,
            (layout.left_w - 16).max(0) as usize,
            (ROW_HEIGHT - 4).max(0) as usize,
            if selected {
                ui::ACCENT_DIM
            } else {
                ui::BG_WINDOW
            },
        );
        rt::draw_text_rgba8888(
            bytes,
            PIXEL_STRIDE,
            layout.left_x + 14,
            row_y as i32 + 4,
            if selected {
                ui::TEXT_PRIMARY
            } else {
                ui::TEXT_SECONDARY
            },
            service_title(entry.service_id),
        );
        let mut meta = FixedLogBuffer::<96>::new();
        let _ = write!(
            &mut meta,
            "v{}  {}  r{}  {}{}{}",
            text_or_dash(&entry.latest_version[..entry.latest_version_len]),
            catalog_meta::category_for(entry.service_id, &entry.category[..entry.category_len]),
            entry.repo_index,
            if entry.installed { "I" } else { "-" },
            if entry.active { "A" } else { "-" },
            if entry.rollback { "R" } else { "-" },
        );
        rt::draw_text_rgba8888(
            bytes,
            PIXEL_STRIDE,
            layout.left_x + 14,
            row_y as i32 + 16,
            ui::TEXT_MUTED,
            str::from_utf8(meta.as_bytes()).unwrap_or(""),
        );
    }
    if state.view_count == 0 {
        rt::draw_text_rgba8888(
            bytes,
            PIXEL_STRIDE,
            layout.left_x + 14,
            layout.list_rows_y + 6,
            ui::TEXT_MUTED,
            "no matching packages",
        );
    }
}

fn draw_button(
    bytes: &mut [u8],
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    color: u32,
    label: &str,
    text_color: u32,
) {
    ui::fill_rgba8888_rect(
        bytes,
        PIXEL_STRIDE,
        BUFFER_WIDTH as usize,
        BUFFER_HEIGHT as usize,
        x0.max(0) as usize,
        y0.max(0) as usize,
        (x1 - x0).max(0) as usize,
        (y1 - y0).max(0) as usize,
        color,
    );
    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, x0 + 8, y0 + 7, text_color, label);
}

fn draw_panel(bytes: &mut [u8], x: i32, y: i32, width: i32, height: i32, color: u32) {
    ui::fill_rgba8888_rect(
        bytes,
        PIXEL_STRIDE,
        BUFFER_WIDTH as usize,
        BUFFER_HEIGHT as usize,
        x.max(0) as usize,
        y.max(0) as usize,
        width.max(0) as usize,
        height.max(0) as usize,
        color,
    );
}

fn draw_chip(bytes: &mut [u8], x: i32, y: i32, label: &str, color: u32, text: u32) {
    let width = (label.len() as i32 * 8 + 12).min(128);
    ui::fill_rgba8888_rect(
        bytes,
        PIXEL_STRIDE,
        BUFFER_WIDTH as usize,
        BUFFER_HEIGHT as usize,
        x.max(0) as usize,
        y.max(0) as usize,
        width.max(0) as usize,
        16,
        color,
    );
    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, x + 6, y + 4, text, label);
}

fn draw_text_fit(bytes: &mut [u8], x: i32, y: i32, color: u32, text: &str, width: i32) {
    let max_chars = (width.max(8) as usize / 8).max(1);
    let mut buffer = FixedLogBuffer::<128>::new();
    let text_bytes = text.as_bytes();
    if text_bytes.len() <= max_chars {
        let _ = buffer.write_str(text);
    } else if max_chars <= 1 {
        let _ = buffer.write_str(".");
    } else if max_chars == 2 {
        let _ = buffer.write_str("..");
    } else {
        let visible = max_chars.saturating_sub(3);
        let slice = &text_bytes[..visible.min(text_bytes.len())];
        let clipped = str::from_utf8(slice).unwrap_or("?");
        let _ = buffer.write_str(clipped);
        let _ = buffer.write_str("...");
    }
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        x,
        y,
        color,
        str::from_utf8(buffer.as_bytes()).unwrap_or(""),
    );
}

const SCREENSHOT_CARD_HEIGHT: i32 = 56;

/// Stylized, honestly-labeled screenshot placeholder. The framebuffer text
/// stack has no image decoding, so a screenshot reference renders as a framed
/// card naming what would be shown instead of pretending to show pixels.
fn draw_screenshot_card(
    bytes: &mut [u8],
    layout: Layout,
    y: i32,
    service_id: rt::ServiceId,
) {
    let screenshot_ref = catalog_meta::screenshot_ref_for(service_id);
    let Some(headline) = catalog_meta::screenshot_placeholder_headline(screenshot_ref) else {
        return;
    };
    let x = layout.right_x + 12;
    let width = layout.right_w - 24;
    draw_panel(bytes, x, y, width, SCREENSHOT_CARD_HEIGHT, ui::ACCENT_DIM);
    draw_panel(
        bytes,
        x + 2,
        y + 2,
        width - 4,
        SCREENSHOT_CARD_HEIGHT - 4,
        ui::BG_WINDOW,
    );
    // Accent spine on the left edge marks this as a media slot.
    draw_panel(bytes, x + 2, y + 2, 5, SCREENSHOT_CARD_HEIGHT - 4, ui::ACCENT);
    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, x + 14, y + 7, ui::TEXT_PRIMARY, headline);
    let mut reference = FixedLogBuffer::<64>::new();
    let _ = write!(&mut reference, "ref: {}", screenshot_ref);
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        x + 14,
        y + 23,
        ui::TEXT_SECONDARY,
        str::from_utf8(reference.as_bytes()).unwrap_or(""),
    );
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        x + 14,
        y + 39,
        ui::TEXT_MUTED,
        "placeholder - image decode not supported yet",
    );
}

/// "Recommended for you" row: offline, deterministic suggestions from the
/// installed set (category popularity + keyword overlap), drawn under the
/// screenshot card when any candidate scores.
fn draw_recommendations(
    bytes: &mut [u8],
    layout: Layout,
    y: i32,
    recommendations: &[catalog_meta::Recommendation],
    state: &AppState,
) {
    if recommendations.is_empty() {
        return;
    }
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        layout.right_x + 12,
        y,
        ui::TEXT_PRIMARY,
        "RECOMMENDED FOR YOU",
    );
    for (row, recommendation) in recommendations.iter().enumerate() {
        let row_y = y + 18 + row as i32 * 28;
        if row_y + 24 > layout.status_y {
            break;
        }
        if recommendation.index >= state.entry_count {
            continue;
        }
        let entry = state.entries[recommendation.index];
        let mut title = FixedLogBuffer::<48>::new();
        let _ = write!(&mut title, "* {}", service_title(entry.service_id));
        rt::draw_text_rgba8888(
            bytes,
            PIXEL_STRIDE,
            layout.right_x + 12,
            row_y,
            ui::ACCENT,
            str::from_utf8(title.as_bytes()).unwrap_or(""),
        );
        draw_text_fit(
            bytes,
            layout.right_x + 12,
            row_y + 12,
            ui::TEXT_MUTED,
            recommendation.reason(),
            layout.detail_text_w,
        );
    }
}

fn draw_status_bar(bytes: &mut [u8], x: i32, y: i32, width: i32, status: &str) {
    ui::fill_rgba8888_rect(
        bytes,
        PIXEL_STRIDE,
        BUFFER_WIDTH as usize,
        BUFFER_HEIGHT as usize,
        x.max(0) as usize,
        y.max(0) as usize,
        width.max(0) as usize,
        STATUS_BAR_HEIGHT as usize,
        ui::BG_WINDOW,
    );
    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, x + 8, y + 8, ui::TEXT_MUTED, status);
}

/// Repository/sources management surface (roadmap 15): list + trust details
/// + two-phase add review. Ledger flows (enable/disable/remove, sideload
/// policy) live in the shell's in-process onboarding ledger with no IPC
/// surface, so the panel shows an honest pointer instead of fake toggles.
fn draw_sources(
    presenter: &mut ui::FirstPresentSurface,
    buffer_slot: u32,
    bytes: &mut [u8],
    layout: Layout,
    state: &AppState,
) -> rt::Result<()> {
    let width = state.width.min(BUFFER_WIDTH) as usize;
    let height = state.height.min(BUFFER_HEIGHT) as usize;
    ui::draw_window_frame_rgba8888(
        bytes,
        PIXEL_STRIDE,
        width,
        height,
        state.focused,
        ui::BG_WINDOW_ALT,
        "SOFTWARE CENTER",
    );
    draw_panel(
        bytes,
        layout.header_x,
        layout.header_y,
        layout.header_w,
        HEADER_HEIGHT,
        ui::BG_PANEL,
    );
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        layout.header_x + 14,
        layout.header_y + 12,
        ui::TEXT_PRIMARY,
        "PACKAGE SOURCES",
    );
    let mut summary = FixedLogBuffer::<64>::new();
    let _ = write!(
        &mut summary,
        "{} sources  S closes  esc cancels",
        state.sources.repo_count,
    );
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        layout.header_x + 14,
        layout.header_y + 28,
        ui::TEXT_SECONDARY,
        str::from_utf8(summary.as_bytes()).unwrap_or(""),
    );
    draw_button(
        bytes,
        layout.sync_x0,
        layout.sync_y0,
        layout.sync_x1,
        layout.sync_y1,
        ui::ACCENT_DIM,
        "SYNC ALL",
        ui::TEXT_PRIMARY,
    );
    draw_panel(
        bytes,
        layout.left_x,
        layout.left_y,
        layout.left_w,
        layout.left_h,
        ui::BG_PANEL,
    );
    draw_panel(
        bytes,
        layout.right_x,
        layout.right_y,
        layout.right_w,
        layout.right_h,
        ui::BG_PANEL,
    );
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        layout.left_x + 12,
        layout.left_y + 10,
        ui::TEXT_PRIMARY,
        "SOURCES",
    );
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        layout.right_x + 12,
        layout.right_y + 10,
        ui::TEXT_PRIMARY,
        if state.sources.in_review() {
            "TRUST REVIEW"
        } else {
            "SOURCE DETAILS"
        },
    );

    draw_source_list(bytes, layout, state);
    if !state.sources.available {
        draw_sources_unavailable(bytes, layout);
    } else if state.sources.in_review() {
        draw_add_review(bytes, layout, state);
    } else {
        draw_source_details(bytes, layout, state);
        draw_add_form(bytes, layout, state);
    }
    draw_status_bar(
        bytes,
        layout.right_x + 12,
        layout.status_y,
        layout.right_w - 24,
        str::from_utf8(&state.status[..state.status_len]).unwrap_or(""),
    );
    presenter.present(
        buffer_slot,
        state.width.min(BUFFER_WIDTH),
        state.height.min(BUFFER_HEIGHT),
    )
}

fn draw_source_list(bytes: &mut [u8], layout: Layout, state: &AppState) {
    let visible_rows = layout.visible_rows();
    for row in 0..visible_rows {
        let position = state.sources.scroll + row;
        if position >= state.sources.repo_count {
            break;
        }
        let entry = &state.sources.repos[position];
        let row_y = layout.list_rows_y as usize + row * ROW_HEIGHT as usize;
        let selected = position == state.sources.selected;
        ui::fill_rgba8888_rect(
            bytes,
            PIXEL_STRIDE,
            BUFFER_WIDTH as usize,
            BUFFER_HEIGHT as usize,
            (layout.left_x + 8) as usize,
            row_y,
            (layout.left_w - 16).max(0) as usize,
            (ROW_HEIGHT - 4).max(0) as usize,
            if selected {
                ui::ACCENT_DIM
            } else {
                ui::BG_WINDOW
            },
        );
        let mut title = FixedLogBuffer::<64>::new();
        let _ = write!(
            &mut title,
            "#{} {}",
            entry.info.repo_index,
            entry.name_text(),
        );
        rt::draw_text_rgba8888(
            bytes,
            PIXEL_STRIDE,
            layout.left_x + 14,
            row_y as i32 + 4,
            if selected {
                ui::TEXT_PRIMARY
            } else {
                ui::TEXT_SECONDARY
            },
            str::from_utf8(title.as_bytes()).unwrap_or(""),
        );
        rt::draw_text_rgba8888(
            bytes,
            PIXEL_STRIDE,
            layout.left_x + 14,
            row_y as i32 + 16,
            ui::TEXT_MUTED,
            repositories::source_row_meta(entry).as_str(),
        );
    }
    if state.sources.repo_count == 0 {
        rt::draw_text_rgba8888(
            bytes,
            PIXEL_STRIDE,
            layout.left_x + 14,
            layout.list_rows_y + 6,
            ui::TEXT_MUTED,
            if state.sources.available {
                "no repositories"
            } else {
                "list unavailable"
            },
        );
    }
}

fn draw_sources_unavailable(bytes: &mut [u8], layout: Layout) {
    let x = layout.right_x + 12;
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        x,
        layout.detail_title_y,
        ui::STATUS_WARN,
        "SOURCES UNAVAILABLE",
    );
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        x,
        layout.detail_title_y + 16,
        ui::TEXT_SECONDARY,
        "package-service did not answer the repository list request.",
    );
    draw_text_fit(
        bytes,
        x,
        layout.detail_body_y,
        ui::TEXT_MUTED,
        repositories::LEDGER_NOTE,
        layout.detail_text_w,
    );
    draw_text_fit(
        bytes,
        x,
        layout.detail_body_y + 14,
        ui::TEXT_MUTED,
        repositories::SIDELOAD_NOTE,
        layout.detail_text_w,
    );
}

fn draw_source_details(bytes: &mut [u8], layout: Layout, state: &AppState) {
    let x = layout.right_x + 12;
    let Some(entry) = state.sources.selected_repo() else {
        rt::draw_text_rgba8888(
            bytes,
            PIXEL_STRIDE,
            x,
            layout.detail_title_y,
            ui::TEXT_PRIMARY,
            "Select a source",
        );
        rt::draw_text_rgba8888(
            bytes,
            PIXEL_STRIDE,
            x,
            layout.detail_title_y + 16,
            ui::TEXT_SECONDARY,
            "Browse the package feed sources and their trust stance.",
        );
        draw_text_fit(
            bytes,
            x,
            layout.detail_body_y,
            ui::TEXT_MUTED,
            LEDGER_NOTE,
            layout.detail_text_w,
        );
        draw_text_fit(
            bytes,
            x,
            layout.detail_body_y + 14,
            ui::TEXT_MUTED,
            SIDELOAD_NOTE,
            layout.detail_text_w,
        );
        return;
    };
    let mut title = FixedLogBuffer::<64>::new();
    let _ = write!(&mut title, "#{} {}", entry.info.repo_index, entry.name_text());
    draw_text_fit(
        bytes,
        x,
        layout.detail_title_y,
        ui::TEXT_PRIMARY,
        str::from_utf8(title.as_bytes()).unwrap_or("SOURCE"),
        layout.detail_text_w,
    );
    draw_text_fit(
        bytes,
        x,
        layout.detail_title_y + 16,
        ui::TEXT_SECONDARY,
        entry.url_text(),
        layout.detail_text_w,
    );
    let mut trust_line = FixedLogBuffer::<128>::new();
    let _ = write!(
        &mut trust_line,
        "trust={} meaning: {}",
        repositories::trust_mode_name(entry.info.trust_mode),
        repositories::trust_meaning(entry.info.trust_mode),
    );
    draw_text_fit(
        bytes,
        x,
        layout.detail_body_y,
        ui::TEXT_SECONDARY,
        str::from_utf8(trust_line.as_bytes()).unwrap_or(""),
        layout.detail_text_w,
    );
    let mut impact_line = FixedLogBuffer::<128>::new();
    let _ = write!(
        &mut impact_line,
        "effect: {}",
        repositories::trust_onboarding_impact(entry.info.trust_mode),
    );
    draw_text_fit(
        bytes,
        x,
        layout.detail_body_y + 14,
        ui::TEXT_SECONDARY,
        str::from_utf8(impact_line.as_bytes()).unwrap_or(""),
        layout.detail_text_w,
    );
    let mut meta = FixedLogBuffer::<96>::new();
    let _ = write!(
        &mut meta,
        "sync={} ch={} ring={} enabled={} pkgs={}",
        repositories::sync_state_name(entry.info.sync_state),
        repositories::repo_channel_name(entry.info.channel),
        repositories::repo_ring_name(entry.info.ring),
        if entry.info.enabled { "yes" } else { "no" },
        entry.info.package_count,
    );
    draw_text_fit(
        bytes,
        x,
        layout.detail_body_y + 28,
        ui::TEXT_SECONDARY,
        str::from_utf8(meta.as_bytes()).unwrap_or(""),
        layout.detail_text_w,
    );
    let mut digests = FixedLogBuffer::<64>::new();
    let _ = write!(
        &mut digests,
        "last={:016x}",
        entry.info.last_digest,
    );
    if entry.info.trust_mode == rt::PackageRepositoryTrustMode::PinnedDigest {
        let _ = write!(&mut digests, "  pinned={:016x}", entry.info.pinned_digest);
    }
    draw_text_fit(
        bytes,
        x,
        layout.detail_body_y + 42,
        ui::TEXT_MUTED,
        str::from_utf8(digests.as_bytes()).unwrap_or(""),
        layout.detail_text_w,
    );
    draw_text_fit(
        bytes,
        x,
        layout.detail_body_y + 58,
        ui::TEXT_MUTED,
        LEDGER_NOTE,
        layout.detail_text_w,
    );
}

fn draw_add_form(bytes: &mut [u8], layout: Layout, state: &AppState) {
    let area = repositories::rects(
        layout,
        state.sources.trust == rt::PackageRepositoryTrustMode::PinnedDigest,
    );
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        area.field_x0,
        area.name_y - 16,
        ui::TEXT_PRIMARY,
        "ADD SOURCE",
    );
    draw_field(bytes, area.field_x0, area.field_x1, area.name_y, "name", AddField::Name, state);
    draw_field(bytes, area.field_x0, area.field_x1, area.url_y, "url", AddField::Url, state);
    let mut trust_value = FixedLogBuffer::<64>::new();
    let _ = write!(
        &mut trust_value,
        "{}  (tab cycles)",
        repositories::trust_mode_name(state.sources.trust),
    );
    draw_field_value(
        bytes,
        area.field_x0,
        area.field_x1,
        area.trust_y,
        "trust",
        str::from_utf8(trust_value.as_bytes()).unwrap_or(""),
        false,
    );
    if state.sources.trust == rt::PackageRepositoryTrustMode::PinnedDigest {
        draw_field(bytes, area.field_x0, area.field_x1, area.digest_y, "digest", AddField::Digest, state);
    }
    draw_button(
        bytes,
        area.primary_x0,
        area.button_y0,
        area.primary_x1,
        area.button_y1,
        ui::ACCENT,
        "ADD REVIEW",
        ui::BG_PANEL,
    );
    draw_button(
        bytes,
        area.secondary_x0,
        area.button_y0,
        area.secondary_x1,
        area.button_y1,
        ui::ACCENT_DIM,
        "SYNC THIS",
        ui::TEXT_PRIMARY,
    );
}

fn draw_field(
    bytes: &mut [u8],
    x0: i32,
    x1: i32,
    y: i32,
    label: &str,
    field: AddField,
    state: &AppState,
) {
    let focused = state.sources.field == field;
    draw_field_value(bytes, x0, x1, y, label, state.sources.field_text(field), focused);
}

fn draw_field_value(
    bytes: &mut [u8],
    x0: i32,
    x1: i32,
    y: i32,
    label: &str,
    value: &str,
    focused: bool,
) {
    ui::fill_rgba8888_rect(
        bytes,
        PIXEL_STRIDE,
        BUFFER_WIDTH as usize,
        BUFFER_HEIGHT as usize,
        x0.max(0) as usize,
        y.max(0) as usize,
        (x1 - x0).max(0) as usize,
        20,
        if focused {
            ui::BG_WINDOW_ALT
        } else {
            ui::BG_WINDOW
        },
    );
    let mut row = FixedLogBuffer::<128>::new();
    let _ = write!(
        &mut row,
        "{}{}: {}",
        if focused { "> " } else { "  " },
        label,
        value,
    );
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        x0 + 4,
        y + 4,
        if focused {
            ui::ACCENT
        } else {
            ui::TEXT_SECONDARY
        },
        str::from_utf8(row.as_bytes()).unwrap_or(""),
    );
}

/// Two-phase add, step 2: the shell's trust-review text rendered verbatim so
/// the GUI confirmation carries the same meaning as `pkg repo add` review.
fn draw_add_review(bytes: &mut [u8], layout: Layout, state: &AppState) {
    let area = repositories::rects(
        layout,
        state.sources.trust == rt::PackageRepositoryTrustMode::PinnedDigest,
    );
    let x = layout.right_x + 12;
    let name = state.sources.field_text(AddField::Name);
    let url = state.sources.field_text(AddField::Url);
    let trust = state.sources.trust;
    let mut lines: [FixedLogBuffer::<128>; 8] = [
        FixedLogBuffer::<128>::new(),
        FixedLogBuffer::<128>::new(),
        FixedLogBuffer::<128>::new(),
        FixedLogBuffer::<128>::new(),
        FixedLogBuffer::<128>::new(),
        FixedLogBuffer::<128>::new(),
        FixedLogBuffer::<128>::new(),
        FixedLogBuffer::<128>::new(),
    ];
    let _ = write!(&mut lines[0], "trust review for third-party repository {}", name);
    let _ = write!(&mut lines[1], "endpoint {}", url);
    let _ = write!(
        &mut lines[2],
        "trust={} meaning: {}",
        repositories::trust_mode_name(trust),
        repositories::trust_meaning(trust),
    );
    let mut used = 3usize;
    if trust == rt::PackageRepositoryTrustMode::PinnedDigest {
        let _ = write!(
            &mut lines[used],
            "pinned digest {:016x}",
            state.sources.parse_digest().unwrap_or(0),
        );
        used += 1;
    }
    let _ = write!(
        &mut lines[used],
        "effect once added: {}",
        repositories::trust_onboarding_impact(trust),
    );
    used += 1;
    let _ = write!(
        &mut lines[used],
        "adds as: channel=stable ring=production enabled=yes",
    );
    used += 1;
    let _ = write!(
        &mut lines[used],
        "packages from this source become installable and update-visible;",
    );
    used += 1;
    let _ = write!(
        &mut lines[used],
        "manage it with pkg repo <enable|disable|remove|status>",
    );
    used += 1;

    let mut y = area.review_y0;
    for line in lines.iter().take(used) {
        if y + 16 > area.review_y1 {
            break;
        }
        draw_text_fit(
            bytes,
            x,
            y,
            ui::TEXT_SECONDARY,
            str::from_utf8(line.as_bytes()).unwrap_or(""),
            layout.detail_text_w,
        );
        y += 16;
    }
    draw_button(
        bytes,
        area.primary_x0,
        area.button_y0,
        area.primary_x1,
        area.button_y1,
        ui::STATUS_OK,
        "CONFIRM ADD",
        ui::BG_PANEL,
    );
    draw_button(
        bytes,
        area.secondary_x0,
        area.button_y0,
        area.secondary_x1,
        area.button_y1,
        ui::STATUS_WARN,
        "CANCEL",
        ui::BG_PANEL,
    );
}
