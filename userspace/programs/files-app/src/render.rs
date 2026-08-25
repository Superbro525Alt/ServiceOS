use core::{fmt::Write as _, str};

use rt::FixedLogBuffer;
use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;

use crate::assoc;
use crate::navigation::{entry_name_bytes, push_selected_path, visible_row_count};
use crate::state::{
    BUFFER_BYTES, BUFFER_HEIGHT, BUFFER_WIDTH, EntryKind, ExplorerEntry, ExplorerState, LIST_X,
    LIST_Y, PIXEL_STRIDE, ROW_HEIGHT, ViewMode,
};

pub(crate) fn render(
    presenter: &mut ui::FirstPresentSurface,
    buffer_slot: u32,
    buffer: &mut rt::MappedMemory,
    state: &ExplorerState,
) -> rt::Result<()> {
    let width = state.width.min(BUFFER_WIDTH) as usize;
    let height = state.height.min(BUFFER_HEIGHT) as usize;
    let bytes = &mut buffer.as_slice_mut()[..BUFFER_BYTES];

    let title = match state.view_mode {
        ViewMode::Directory => "FILES",
        ViewMode::Recent => "RECENT",
    };
    ui::draw_window_frame_rgba8888(
        bytes,
        PIXEL_STRIDE,
        width,
        height,
        state.focused,
        ui::BG_WINDOW_ALT,
        title,
    );
    draw_header(bytes, state);
    match state.view_mode {
        ViewMode::Directory => draw_list(bytes, state),
        ViewMode::Recent => draw_recent(bytes, state),
    }
    draw_footer(bytes, state);

    presenter.present(
        buffer_slot,
        state.width.min(BUFFER_WIDTH),
        state.height.min(BUFFER_HEIGHT),
    )
}

fn draw_header(bytes: &mut [u8], state: &ExplorerState) {
    let mut path_line = FixedLogBuffer::<128>::new();
    let _ = path_line.write_fmt(format_args!("PATH "));
    if state.view_mode == ViewMode::Recent {
        let _ = path_line.write_fmt(format_args!("RECENT {} FILES", state.recent.len()));
    } else if state.current_path_len == 0 {
        let _ = path_line.write_fmt(format_args!("/"));
    } else if let Ok(path) = str::from_utf8(&state.current_path[..state.current_path_len]) {
        let _ = path_line.write_fmt(format_args!("/{path}"));
    } else {
        let _ = path_line.write_fmt(format_args!("/INVALID"));
    }
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        LIST_X as i32,
        ui::TITLEBAR_HEIGHT as i32 + 10,
        ui::TEXT_PRIMARY,
        str::from_utf8(path_line.as_bytes()).unwrap_or("PATH /"),
    );
}

fn draw_list(bytes: &mut [u8], state: &ExplorerState) {
    let width = state.width.min(BUFFER_WIDTH) as usize;
    let height = state.height.min(BUFFER_HEIGHT) as usize;
    let list_height = height.saturating_sub(LIST_Y + crate::state::LIST_BOTTOM_MARGIN);
    ui::fill_rgba8888_rect(
        bytes,
        PIXEL_STRIDE,
        width,
        height,
        LIST_X,
        LIST_Y,
        width.saturating_sub(LIST_X * 2),
        list_height,
        ui::BG_WINDOW,
    );

    if state.loading_initial_directory {
        draw_note(bytes, "LOADING", ui::TEXT_MUTED);
        return;
    }

    if state.load_failed {
        draw_note(bytes, "LIST FAILED", ui::STATUS_WARN);
        return;
    }

    if state.entry_count == 0 {
        draw_note(bytes, "EMPTY", ui::TEXT_MUTED);
        return;
    }

    let visible = visible_row_count(state);
    for row in 0..visible {
        let index = state.scroll_offset + row;
        if index >= state.entry_count {
            break;
        }
        let y = LIST_Y + row * ROW_HEIGHT;
        let selected = index == state.selected_index;
        let drag_source = state.dragging && state.press.is_some_and(|press| press.index == index);
        if selected || drag_source {
            ui::fill_rgba8888_rect(
                bytes,
                PIXEL_STRIDE,
                width,
                height,
                LIST_X + 4,
                y + 1,
                width.saturating_sub(LIST_X * 2 + 8),
                ROW_HEIGHT.saturating_sub(1),
                if drag_source {
                    ui::ACCENT
                } else {
                    ui::ACCENT_DIM
                },
            );
        }

        let entry = state.entries[index];
        let color = match entry.kind {
            EntryKind::Parent => ui::STATUS_WARN,
            EntryKind::Directory => ui::ACCENT,
            EntryKind::File => {
                if drag_source {
                    ui::BG_WINDOW_ALT
                } else if selected {
                    ui::TEXT_PRIMARY
                } else {
                    ui::TEXT_SECONDARY
                }
            }
        };
        draw_entry_label(bytes, entry, (LIST_X + 8) as i32, (y + 4) as i32, color);
    }
}

fn draw_recent(bytes: &mut [u8], state: &ExplorerState) {
    let width = state.width.min(BUFFER_WIDTH) as usize;
    let height = state.height.min(BUFFER_HEIGHT) as usize;
    let list_height = height.saturating_sub(LIST_Y + crate::state::LIST_BOTTOM_MARGIN);
    ui::fill_rgba8888_rect(
        bytes,
        PIXEL_STRIDE,
        width,
        height,
        LIST_X,
        LIST_Y,
        width.saturating_sub(LIST_X * 2),
        list_height,
        ui::BG_WINDOW,
    );

    if state.recent.len() == 0 {
        draw_note(bytes, "NO RECENT FILES", ui::TEXT_MUTED);
        return;
    }

    let visible = visible_row_count(state);
    for row in 0..visible {
        let index = state
            .scroll_offset
            .min(state.recent.len().saturating_sub(1))
            + row;
        let Some(path) = state.recent.get(index) else {
            break;
        };
        let y = LIST_Y + row * ROW_HEIGHT;
        if index == state.recent_sel {
            ui::fill_rgba8888_rect(
                bytes,
                PIXEL_STRIDE,
                width,
                height,
                LIST_X + 4,
                y + 1,
                width.saturating_sub(LIST_X * 2 + 8),
                ROW_HEIGHT.saturating_sub(1),
                ui::ACCENT_DIM,
            );
        }
        let mut label = FixedLogBuffer::<128>::new();
        crate::recent::RecentRing::label(&mut label, path);
        let color = if index == state.recent_sel {
            ui::TEXT_PRIMARY
        } else {
            ui::TEXT_SECONDARY
        };
        rt::draw_text_rgba8888(
            bytes,
            PIXEL_STRIDE,
            (LIST_X + 8) as i32,
            (y + 4) as i32,
            color,
            str::from_utf8(label.as_bytes()).unwrap_or("INVALID"),
        );
    }
}

fn draw_note(bytes: &mut [u8], text: &str, color: u32) {
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        LIST_X as i32 + 6,
        LIST_Y as i32 + 8,
        color,
        text,
    );
}

fn draw_footer(bytes: &mut [u8], state: &ExplorerState) {
    let height = state.height.min(BUFFER_HEIGHT) as usize;
    let footer_y = height.saturating_sub(14);
    let mut footer = FixedLogBuffer::<128>::new();
    match state.view_mode {
        ViewMode::Recent => {
            let _ = write!(
                footer,
                "{} of {}  [R] BACK",
                state.recent_sel.min(state.recent.len().saturating_sub(1)) + 1,
                state.recent.len(),
            );
        }
        ViewMode::Directory => {
            if state.dragging {
                let name = state
                    .press
                    .filter(|press| press.index < state.entry_count)
                    .map(|press| entry_name_bytes(&state.entries[press.index]))
                    .and_then(|name| str::from_utf8(name).ok())
                    .unwrap_or("FILE");
                let _ = write!(footer, "DRAGGING {name}");
            } else if state.entry_count == 0 {
                let _ = write!(footer, "0 entries");
            } else {
                let selected = state.entries[state.selected_index.min(state.entry_count - 1)];
                let _ = write!(
                    footer,
                    "{} of {}  ",
                    state.selected_index.min(state.entry_count - 1) + 1,
                    state.entry_count,
                );
                push_selected_path(&mut footer, selected);
                append_open_hint(&mut footer, state);
            }
        }
    }
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        LIST_X as i32,
        footer_y as i32,
        ui::TEXT_MUTED,
        str::from_utf8(footer.as_bytes()).unwrap_or("FILES"),
    );
}

/// Footer suffix describing the routing policy for the selected file:
/// default app plus the active open-with pick, e.g. `txt>TERM PICK MON`.
fn append_open_hint(footer: &mut FixedLogBuffer<128>, state: &ExplorerState) {
    let selected = state.entries[state.selected_index.min(state.entry_count - 1)];
    if !matches!(selected.kind, EntryKind::File) {
        return;
    }
    let ext = assoc::extension_of(&selected.path[..selected.path_len]);
    let mut ext_buf = [0u8; 12];
    if ext.len() > ext_buf.len() {
        return;
    }
    ext_buf[..ext.len()].copy_from_slice(ext);
    let ext_slice = &ext_buf[..ext.len()];
    let default_app = assoc::route_app(ext_slice, &state.assoc, None);
    match state.open_with_pick {
        Some(_) => {
            let pick = pick_label(state);
            let _ = write!(
                footer,
                "  {}>{} PICK",
                display_ext(ext_slice),
                assoc::app_label(pick)
            );
        }
        None => {
            let _ = write!(
                footer,
                "  {}>{} [O]",
                display_ext(ext_slice),
                assoc::app_label(default_app)
            );
        }
    }
}

fn display_ext(ext: &[u8]) -> &str {
    if ext.is_empty() {
        return "*";
    }
    str::from_utf8(ext).unwrap_or("*")
}

fn pick_label(state: &ExplorerState) -> rt::DesktopAppId {
    let candidates = crate::control::current_candidates(state);
    let pick = state.open_with_pick.unwrap_or(0);
    candidates
        .0
        .get(pick)
        .copied()
        .flatten()
        .filter(|_| pick < candidates.1)
        .unwrap_or_else(|| {
            selected_ext(state)
                .map(|(len, ext)| assoc::route_app(&ext[..len], &state.assoc, None))
                .unwrap_or(rt::DesktopAppId::Files)
        })
}

fn selected_ext(state: &ExplorerState) -> Option<(usize, [u8; 16])> {
    let selected = state.entries.get(state.selected_index)?;
    if !matches!(selected.kind, EntryKind::File) {
        return None;
    }
    let mut ext = [0u8; 16];
    let raw = assoc::extension_of(&selected.path[..selected.path_len]);
    if raw.len() > ext.len() {
        return None;
    }
    ext[..raw.len()].copy_from_slice(raw);
    Some((raw.len(), ext))
}

fn draw_entry_label(bytes: &mut [u8], entry: ExplorerEntry, x: i32, y: i32, color: u32) {
    let mut label = FixedLogBuffer::<96>::new();
    if entry.kind == EntryKind::Parent {
        let _ = write!(label, "UP   ..");
    } else {
        match entry.kind {
            EntryKind::Directory => {
                let _ = write!(label, "DIR  ");
            }
            EntryKind::File => {
                let _ = write!(label, "FILE ");
            }
            EntryKind::Parent => {}
        }
        if let Ok(name) = str::from_utf8(entry_name_bytes(&entry)) {
            let _ = write!(label, "{name}");
            if entry.kind == EntryKind::Directory {
                let _ = write!(label, "/");
            }
        } else {
            let _ = write!(label, "INVALID");
        }
    }
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        x,
        y,
        color,
        str::from_utf8(label.as_bytes()).unwrap_or("INVALID"),
    );
}
