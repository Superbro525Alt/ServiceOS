use core::fmt::Write;

use super::*;

pub(crate) fn render(
    surface_handle: rt::Handle,
    buffer_slot: u32,
    mapped: &mut rt::MappedMemory,
    state: &TerminalState,
) -> rt::Result<()> {
    let width = state.width.min(BUFFER_WIDTH) as usize;
    let height = state.height.min(BUFFER_HEIGHT) as usize;
    let bytes = mapped.as_slice_mut();
    fill_rect(bytes, 0, 0, width, height, THEMES[state.theme_index].bg);
    let theme = &THEMES[state.theme_index];
    fill_rect(
        bytes,
        0,
        ui::TITLEBAR_HEIGHT as usize,
        width,
        height.saturating_sub(ui::TITLEBAR_HEIGHT as usize),
        theme.bg,
    );

    draw_titlebar(bytes, width, theme);
    draw_tab_strip(bytes, width, state, theme);
    draw_terminal_contents(bytes, width, height, state, theme);
    rt::surface_present_buffer_slot(
        surface_handle,
        buffer_slot,
        0,
        0,
        state.width.min(BUFFER_WIDTH),
        state.height.min(BUFFER_HEIGHT),
    )
}

fn draw_titlebar(bytes: &mut [u8], width: usize, theme: &Theme) {
    let close_x = width as i32 - ui::WINDOW_BUTTON_RIGHT_MARGIN - ui::WINDOW_BUTTON_SIZE as i32;
    let minimize_x = close_x - ui::WINDOW_BUTTON_GAP - ui::WINDOW_BUTTON_SIZE as i32;
    let maximize_x = minimize_x - ui::WINDOW_BUTTON_GAP - ui::WINDOW_BUTTON_SIZE as i32;
    fill_rect(
        bytes,
        maximize_x.max(0) as usize,
        ui::WINDOW_BUTTON_TOP.max(0) as usize,
        ui::WINDOW_BUTTON_SIZE as usize,
        ui::WINDOW_BUTTON_SIZE as usize,
        theme.panel_alt,
    );
    fill_rect(
        bytes,
        minimize_x.max(0) as usize,
        ui::WINDOW_BUTTON_TOP.max(0) as usize,
        ui::WINDOW_BUTTON_SIZE as usize,
        ui::WINDOW_BUTTON_SIZE as usize,
        theme.muted,
    );
    fill_rect(
        bytes,
        close_x.max(0) as usize,
        ui::WINDOW_BUTTON_TOP.max(0) as usize,
        ui::WINDOW_BUTTON_SIZE as usize,
        ui::WINDOW_BUTTON_SIZE as usize,
        ui::STATUS_WARN,
    );
    fill_rect(
        bytes,
        (maximize_x + 3).max(0) as usize,
        (ui::WINDOW_BUTTON_TOP + 3).max(0) as usize,
        6,
        6,
        theme.bg,
    );
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        minimize_x + 3,
        ui::WINDOW_BUTTON_TOP + 2,
        theme.bg,
        "_",
    );
    rt::draw_text_rgba8888(
        bytes,
        PIXEL_STRIDE,
        close_x + 3,
        ui::WINDOW_BUTTON_TOP + 2,
        theme.bg,
        "X",
    );
    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, 10, 9, ui::TEXT_PRIMARY, "TERMINAL");
    rt::draw_text_rgba8888(bytes, PIXEL_STRIDE, 76, 9, theme.muted, theme.name);
}

fn draw_tab_strip(bytes: &mut [u8], width: usize, state: &TerminalState, theme: &Theme) {
    let strip_y = ui::TITLEBAR_HEIGHT as usize + 4;
    fill_rect(
        bytes,
        CONTENT_PADDING_X,
        strip_y,
        width.saturating_sub(CONTENT_PADDING_X * 2),
        TAB_STRIP_HEIGHT,
        theme.panel_alt,
    );
    for index in 0..MAX_TABS {
        if !state.tabs[index].occupied {
            continue;
        }
        let x = CONTENT_PADDING_X + index * TAB_WIDTH;
        let fill = if index == state.active_tab {
            ui::ACCENT
        } else {
            theme.panel
        };
        fill_rect(
            bytes,
            x,
            strip_y,
            TAB_WIDTH.saturating_sub(4),
            TAB_STRIP_HEIGHT.saturating_sub(2),
            fill,
        );
        if state.tabs[index].title_len > 0 {
            let text = core::str::from_utf8(&state.tabs[index].title[..state.tabs[index].title_len])
                .unwrap_or("TAB");
            rt::draw_text_rgba8888(
                bytes,
                PIXEL_STRIDE,
                (x + 8) as i32,
                (strip_y + 4) as i32,
                ui::TEXT_PRIMARY,
                text,
            );
        } else {
            let mut label = rt::FixedLogBuffer::<16>::new();
            let _ = write!(&mut label, "SHELL {}", state.tabs[index].session_id);
            if let Ok(text) = core::str::from_utf8(label.as_bytes()) {
                rt::draw_text_rgba8888(
                    bytes,
                    PIXEL_STRIDE,
                    (x + 8) as i32,
                    (strip_y + 4) as i32,
                    ui::TEXT_PRIMARY,
                    text,
                );
            }
        }
    }
}

fn draw_terminal_contents(
    bytes: &mut [u8],
    width: usize,
    height: usize,
    state: &TerminalState,
    theme: &Theme,
) {
    let Some(tab) = crate::tabs::active_tab_ref(state) else {
        return;
    };
    let start_x = CONTENT_PADDING_X;
    let start_y = ui::TITLEBAR_HEIGHT as usize + TAB_STRIP_HEIGHT + CONTENT_PADDING_Y;
    let visible_rows = state.rows.min(MAX_SCROLLBACK_LINES);
    let first_line = first_visible_line(tab, visible_rows);
    let lines = unsafe { GRIDS.tab(state.active_tab) };

    for row in 0..visible_rows {
        let grid_line = first_line + row;
        if grid_line >= MAX_SCROLLBACK_LINES {
            break;
        }
        let y = start_y + row * CELL_HEIGHT;
        if y + CELL_HEIGHT >= height {
            break;
        }
        for col in 0..state.columns {
            let x = start_x + col * CELL_WIDTH;
            if x + CELL_WIDTH >= width {
                break;
            }
            let cell = lines[grid_line][col];
            let (fg, bg) = resolve_cell_colors(cell, theme);
            let highlight = selection_contains(state.selection, grid_line, col);
            if highlight || bg != theme.bg {
                fill_rect(
                    bytes,
                    x,
                    y,
                    CELL_WIDTH,
                    CELL_HEIGHT,
                    if highlight { theme.selection } else { bg },
                );
            }
            if cell.ch != b' ' {
                rt::draw_glyph_rgba8888(
                    bytes,
                    PIXEL_STRIDE,
                    x as i32,
                    y as i32,
                    fg,
                    rt::normalize_bitmap_glyph(cell.ch),
                );
            }
        }
    }

    if tab.scroll_offset > 0 {
        let mut status = rt::FixedLogBuffer::<32>::new();
        let _ = write!(&mut status, "SCROLL -{}", tab.scroll_offset);
        if let Ok(label) = core::str::from_utf8(status.as_bytes()) {
            let label_width = label.len() * rt::BITMAP_GLYPH_ADVANCE;
            let label_x = width.saturating_sub(label_width + CONTENT_PADDING_X);
            rt::draw_text_rgba8888(
                bytes,
                PIXEL_STRIDE,
                label_x as i32,
                start_y as i32,
                theme.muted,
                label,
            );
        }
    }

    if state.focused && tab.scroll_offset == 0 && tab.cursor_visible {
        let cursor_visible_row = tab.cursor_line.saturating_sub(first_line);
        if cursor_visible_row < visible_rows && tab.cursor_col < state.columns {
            let cursor_x = start_x + tab.cursor_col * CELL_WIDTH;
            let cursor_y = start_y + cursor_visible_row * CELL_HEIGHT;
            fill_rect(
                bytes,
                cursor_x,
                cursor_y + CELL_HEIGHT - 2,
                CELL_WIDTH,
                2,
                ui::ACCENT,
            );
        }
    }
}

fn resolve_cell_colors(cell: Cell, theme: &Theme) -> (u32, u32) {
    let mut fg = if cell.fg == COLOR_DEFAULT {
        theme.fg
    } else {
        theme.ansi[(cell.fg - 1).min(15) as usize]
    };
    let mut bg = if cell.bg == COLOR_DEFAULT {
        theme.bg
    } else {
        theme.ansi[(cell.bg - 1).min(15) as usize]
    };
    if cell.flags & CELL_FLAG_BOLD != 0 && cell.fg != COLOR_DEFAULT && cell.fg <= 8 {
        fg = theme.ansi[(cell.fg + 7).min(16) as usize - 1];
    }
    if cell.flags & CELL_FLAG_INVERSE != 0 {
        core::mem::swap(&mut fg, &mut bg);
    }
    (fg, bg)
}

fn selection_contains(selection: Option<Selection>, line: usize, col: usize) -> bool {
    let Some(selection) = selection else {
        return false;
    };
    let (start, end) = ordered_selection(selection);
    if line < start.line || line > end.line {
        return false;
    }
    if start.line == end.line {
        return line == start.line && col >= start.col && col <= end.col;
    }
    if line == start.line {
        return col >= start.col;
    }
    if line == end.line {
        return col <= end.col;
    }
    true
}

fn ordered_selection(selection: Selection) -> (CellPos, CellPos) {
    if selection.anchor.line < selection.focus.line
        || (selection.anchor.line == selection.focus.line && selection.anchor.col <= selection.focus.col)
    {
        (selection.anchor, selection.focus)
    } else {
        (selection.focus, selection.anchor)
    }
}

fn first_visible_line(tab: &TerminalTab, visible_rows: usize) -> usize {
    let scroll_offset = tab
        .scroll_offset
        .min(tab.line_count.saturating_sub(visible_rows));
    tab.line_count.saturating_sub(visible_rows + scroll_offset)
}

pub(crate) fn clamp_scroll_offset(tab: &mut TerminalTab, rows: usize) {
    let max_offset = tab.line_count.saturating_sub(rows.min(MAX_SCROLLBACK_LINES));
    tab.scroll_offset = tab.scroll_offset.min(max_offset);
}

pub(crate) fn scroll_up_view(tab: &mut TerminalTab, lines: usize, rows: usize) {
    let max_offset = tab.line_count.saturating_sub(rows.min(MAX_SCROLLBACK_LINES));
    tab.scroll_offset = tab.scroll_offset.saturating_add(lines).min(max_offset);
}

pub(crate) fn scroll_down_view(tab: &mut TerminalTab, lines: usize) {
    tab.scroll_offset = tab.scroll_offset.saturating_sub(lines);
}

pub(crate) fn copy_selection(state: &mut TerminalState) {
    let len = copy_selection_into_local_buffer(state);
    if len > 0 && state.clipboard_handle != rt::INVALID_HANDLE {
        let _ = rt::clipboard_write(state.clipboard_handle, &state.clipboard[..len]);
    }
}

fn copy_selection_into_local_buffer(state: &mut TerminalState) -> usize {
    let Some(selection) = state.selection else {
        state.clipboard_len = 0;
        return 0;
    };
    let lines = unsafe { GRIDS.tab(state.active_tab) };
    let (start, end) = ordered_selection(selection);
    let mut len = 0usize;
    for line in start.line..=end.line {
        let start_col = if line == start.line { start.col } else { 0 };
        let end_col = if line == end.line {
            end.col
        } else {
            state.columns.saturating_sub(1)
        };
        for col in start_col..=end_col.min(MAX_COLS.saturating_sub(1)) {
            if len >= state.clipboard.len() {
                break;
            }
            state.clipboard[len] = lines[line.min(MAX_SCROLLBACK_LINES - 1)][col].ch;
            len += 1;
        }
        if line != end.line && len < state.clipboard.len() {
            state.clipboard[len] = b'\n';
            len += 1;
        }
        if len >= state.clipboard.len() {
            break;
        }
    }
    state.clipboard_len = len;
    len
}

pub(crate) fn paste_clipboard(state: &mut TerminalState) -> rt::Result<()> {
    let mut len = state.clipboard_len;
    if state.clipboard_handle != rt::INVALID_HANDLE {
        if let Ok(read) = rt::clipboard_read(state.clipboard_handle, &mut state.clipboard) {
            state.clipboard_len = read;
            len = read;
        }
    }
    if len > 0 {
        let session_handle = {
            let Some(tab) = crate::tabs::active_tab_mut(state) else {
                return Ok(());
            };
            tab.scroll_offset = 0;
            tab.session_handle
        };
        state.selection = None;
        rt::terminal_session_send_input(session_handle, &state.clipboard[..len])?;
    }
    Ok(())
}

pub(crate) fn tab_strip_hit_index(x: i32, y: i32) -> Option<usize> {
    let strip_y = ui::TITLEBAR_HEIGHT as i32 + 4;
    if y < strip_y || y >= strip_y + TAB_STRIP_HEIGHT as i32 {
        return None;
    }
    let local_x = x - CONTENT_PADDING_X as i32;
    if local_x < 0 {
        return None;
    }
    let index = (local_x as usize) / TAB_WIDTH;
    (index < MAX_TABS).then_some(index)
}

pub(crate) fn pointer_to_cell(state: &TerminalState, x: i32, y: i32) -> Option<CellPos> {
    let tab = crate::tabs::active_tab_ref(state)?;
    let start_x = CONTENT_PADDING_X as i32;
    let start_y = (ui::TITLEBAR_HEIGHT as usize + TAB_STRIP_HEIGHT + CONTENT_PADDING_Y) as i32;
    if x < start_x || y < start_y {
        return None;
    }
    let col = ((x - start_x) as usize) / CELL_WIDTH;
    let row = ((y - start_y) as usize) / CELL_HEIGHT;
    if col >= state.columns || row >= state.rows {
        return None;
    }
    let line = first_visible_line(tab, state.rows.min(MAX_SCROLLBACK_LINES)) + row;
    Some(CellPos { line, col })
}

fn fill_rect(bytes: &mut [u8], x: usize, y: usize, width: usize, height: usize, rgb: u32) {
    let end_x = (x + width).min(BUFFER_WIDTH as usize);
    let end_y = (y + height).min(BUFFER_HEIGHT as usize);
    for py in y..end_y {
        for px in x..end_x {
            rt::set_pixel_rgba8888(bytes, BUFFER_WIDTH as usize, px, py, rgb);
        }
    }
}
