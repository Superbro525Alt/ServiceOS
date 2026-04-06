use core::{fmt::Write, str};

use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;
use rt::FixedLogBuffer;

use crate::actions::{
    action_label, category_chip_label, channel_label, ring_label, text_or_dash, trust_badge,
};
use crate::state::{
    compute_layout, installed_count, selected_entry, service_title, AppState, CatalogEntry, Layout,
    BUFFER_BYTES, BUFFER_HEIGHT, BUFFER_WIDTH, HEADER_HEIGHT, MAX_SOURCE_BYTES, PIXEL_STRIDE,
    ROW_HEIGHT, STATUS_BAR_HEIGHT,
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
    let mut detail0 = FixedLogBuffer::<64>::new();
    let mut detail1 = FixedLogBuffer::<80>::new();
    let mut detail2 = FixedLogBuffer::<80>::new();
    let mut detail3 = FixedLogBuffer::<96>::new();
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
            let _ = write!(
                &mut detail1,
                "{}  repo={}  {}",
                text_or_dash(&entry.summary[..entry.summary_len]),
                provenance.repo_index,
                trust_badge(provenance.trust_state),
            );
            let _ = write!(
                &mut detail1,
                ""
            );
            let _ = write!(
                &mut detail2,
                "latest={}  installed={}  active={}",
                text_or_dash(&latest[..provenance.latest_version_len]),
                text_or_dash(&installed[..provenance.installed_version_len]),
                text_or_dash(&active[..provenance.active_version_len]),
            );
            let _ = write!(
                &mut detail3,
                "channel={}  ring={}  rollback={}",
                channel_label(provenance.channel),
                ring_label(provenance.ring),
                text_or_dash(&rollback[..provenance.rollback_version_len]),
            );
            let _ = source;
        } else {
            let _ = write!(&mut detail0, "{}", service_title(entry.service_id));
            let _ = write!(&mut detail1, "{}", text_or_dash(&entry.summary[..entry.summary_len]));
            let _ = write!(
                &mut detail2,
                "latest={}",
                text_or_dash(&entry.latest_version[..entry.latest_version_len])
            );
            let _ = write!(
                &mut detail3,
                "category={}",
                text_or_dash(&entry.category[..entry.category_len])
            );
        }
    } else {
        let _ = write!(&mut detail0, "Select a package");
        let _ = write!(&mut detail1, "Browse the catalog and inspect trust before installing.");
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
    draw_panel(bytes, layout.left_x, layout.left_y, layout.left_w, layout.left_h, ui::BG_PANEL);
    draw_panel(bytes, layout.right_x, layout.right_y, layout.right_w, layout.right_h, ui::BG_PANEL);
    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, layout.left_x + 12, layout.left_y + 10, ui::TEXT_PRIMARY, "CATALOG");
    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, layout.right_x + 12, layout.right_y + 10, ui::TEXT_PRIMARY, "DETAILS");
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
    );
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
    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, layout.header_x + 14, layout.header_y + 12, ui::TEXT_PRIMARY, "DISCOVER AND MANAGE SOFTWARE");
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
) {
    let meta_x = layout.right_x + 12;
    let title_y = layout.detail_title_y;
    draw_text_fit(bytes, meta_x, title_y, ui::TEXT_PRIMARY, detail0, layout.detail_text_w);
    draw_text_fit(bytes, meta_x, title_y + 16, ui::TEXT_SECONDARY, detail1, layout.detail_text_w);
    if let Some(entry) = entry {
        draw_chip(
            bytes,
            meta_x,
            layout.detail_chip_y,
            category_chip_label(&entry),
            ui::ACCENT_DIM,
            ui::TEXT_PRIMARY,
        );
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
            draw_chip(bytes, layout.install_x0, active_y, "ACTIVE", ui::ACCENT, ui::BG_PANEL);
        }
    }
    draw_text_fit(bytes, meta_x, layout.detail_body_y, ui::TEXT_SECONDARY, detail2, layout.detail_text_w);
    draw_text_fit(bytes, meta_x, layout.detail_body_y + 14, ui::TEXT_SECONDARY, detail3, layout.detail_text_w);
    draw_status_bar(bytes, layout.right_x + 12, layout.status_y, layout.right_w - 24, status);
}

fn draw_list(bytes: &mut [u8], layout: Layout, state: &AppState) {
    let visible_rows = layout.visible_rows();
    for row in 0..visible_rows {
        let entry_index = state.scroll_offset + row;
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
            if selected { ui::ACCENT_DIM } else { ui::BG_WINDOW },
        );
        rt::draw_text_rgba8888(
            bytes,
            PIXEL_STRIDE,
            layout.left_x + 14,
            row_y as i32 + 4,
            if selected { ui::TEXT_PRIMARY } else { ui::TEXT_SECONDARY },
            service_title(entry.service_id),
        );
        let mut meta = FixedLogBuffer::<96>::new();
        let _ = write!(
            &mut meta,
            "v{}  {}  r{}  {}{}{}",
            text_or_dash(&entry.latest_version[..entry.latest_version_len]),
            text_or_dash(&entry.category[..entry.category_len]),
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
