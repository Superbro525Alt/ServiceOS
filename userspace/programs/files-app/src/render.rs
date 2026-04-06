use core::{fmt::Write, str};

use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;
use rt::FixedLogBuffer;

use crate::navigation::{entry_name_bytes, push_selected_path, visible_row_count};
use crate::state::{
    EntryKind, ExplorerEntry, ExplorerState, BUFFER_BYTES, BUFFER_HEIGHT, BUFFER_WIDTH, LIST_X,
    LIST_Y, PIXEL_STRIDE, ROW_HEIGHT,
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

    ui::draw_window_frame_rgba8888(
        bytes,
        PIXEL_STRIDE,
        width,
        height,
        state.focused,
        ui::BG_WINDOW_ALT,
        "FILES",
    );
    draw_header(bytes, state);
    draw_list(bytes, state);
    draw_footer(bytes, state);

    presenter.present(
        buffer_slot,
        state.width.min(BUFFER_WIDTH),
        state.height.min(BUFFER_HEIGHT),
    )
}

fn draw_header(bytes: &mut [u8], state: &ExplorerState) {
    let mut path_line = FixedLogBuffer::<128>::new();
    let _ = write!(&mut path_line, "PATH ");
    if state.current_path_len == 0 {
        let _ = write!(&mut path_line, "/");
    } else if let Ok(path) = str::from_utf8(&state.current_path[..state.current_path_len]) {
        let _ = write!(&mut path_line, "/{}", path);
    } else {
        let _ = write!(&mut path_line, "/INVALID");
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

    if state.load_failed {
        rt::draw_text_rgba8888(
            bytes,
            PIXEL_STRIDE,
            LIST_X as i32 + 6,
            LIST_Y as i32 + 8,
            ui::STATUS_WARN,
            "LIST FAILED",
        );
        return;
    }

    if state.entry_count == 0 {
        rt::draw_text_rgba8888(
            bytes,
            PIXEL_STRIDE,
            LIST_X as i32 + 6,
            LIST_Y as i32 + 8,
            ui::TEXT_MUTED,
            "EMPTY",
        );
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
        if selected {
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

        let entry = state.entries[index];
        let color = match entry.kind {
            EntryKind::Parent => ui::STATUS_WARN,
            EntryKind::Directory => ui::ACCENT,
            EntryKind::File => {
                if selected { ui::TEXT_PRIMARY } else { ui::TEXT_SECONDARY }
            }
        };
        draw_entry_label(bytes, entry, (LIST_X + 8) as i32, (y + 4) as i32, color);
    }
}

fn draw_footer(bytes: &mut [u8], state: &ExplorerState) {
    let height = state.height.min(BUFFER_HEIGHT) as usize;
    let footer_y = height.saturating_sub(14);
    let mut footer = FixedLogBuffer::<128>::new();
    if state.entry_count == 0 {
        let _ = write!(&mut footer, "0 entries");
    } else {
        let selected = state.entries[state.selected_index.min(state.entry_count - 1)];
        let _ = write!(
            &mut footer,
            "{} of {}  ",
            state.selected_index.min(state.entry_count - 1) + 1,
            state.entry_count,
        );
        push_selected_path(&mut footer, selected);
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

fn draw_entry_label(bytes: &mut [u8], entry: ExplorerEntry, x: i32, y: i32, color: u32) {
    let mut label = FixedLogBuffer::<96>::new();
    if entry.kind == EntryKind::Parent {
        let _ = write!(&mut label, "UP   ..");
    } else {
        match entry.kind {
            EntryKind::Directory => {
                let _ = write!(&mut label, "DIR  ");
            }
            EntryKind::File => {
                let _ = write!(&mut label, "FILE ");
            }
            EntryKind::Parent => {}
        }
        if let Ok(name) = str::from_utf8(entry_name_bytes(&entry)) {
            let _ = write!(&mut label, "{name}");
            if entry.kind == EntryKind::Directory {
                let _ = write!(&mut label, "/");
            }
        } else {
            let _ = write!(&mut label, "INVALID");
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
