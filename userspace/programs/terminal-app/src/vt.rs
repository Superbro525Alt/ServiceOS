use super::*;

pub(crate) fn apply_output(state: &mut TerminalState, tab_index: usize, bytes: &[u8]) {
    let columns = state.columns;
    let tab = &mut state.tabs[tab_index];
    tab.scroll_offset = 0;
    for byte in bytes.iter().copied() {
        match tab.parse_state {
            ParseState::Ground => match byte {
                b'\x1b' => tab.parse_state = ParseState::Esc,
                b'\r' => tab.cursor_col = 0,
                b'\n' => explicit_new_line(tab, tab_index),
                b'\t' => advance_tab_stop(tab, columns, tab_index),
                0x20..=0x7e => put_char(tab, columns, tab_index, byte),
                _ => {}
            },
            ParseState::Esc => match byte {
                b'[' => {
                    tab.parse_state = ParseState::Csi;
                    tab.csi_params = [0; 8];
                    tab.csi_count = 1;
                    tab.csi_private = false;
                }
                b']' => {
                    tab.parse_state = ParseState::Osc;
                    tab.osc_len = 0;
                    tab.osc_esc_pending = false;
                }
                b'7' => {
                    tab.saved_cursor_line = tab.cursor_line;
                    tab.saved_cursor_col = tab.cursor_col;
                    reset_escape(tab);
                }
                b'8' => {
                    tab.cursor_line = tab.saved_cursor_line.min(tab.line_count.saturating_sub(1));
                    tab.cursor_col = tab.saved_cursor_col.min(columns.saturating_sub(1));
                    reset_escape(tab);
                }
                b'c' => {
                    reset_terminal_tab(tab, tab_index);
                    reset_escape(tab);
                }
                _ => reset_escape(tab),
            },
            ParseState::Csi => match byte {
                b'?' if tab.csi_count == 1 && tab.csi_params[0] == 0 => tab.csi_private = true,
                b'0'..=b'9' => {
                    let index = tab
                        .csi_count
                        .saturating_sub(1)
                        .min(tab.csi_params.len() - 1);
                    tab.csi_params[index] = tab.csi_params[index]
                        .saturating_mul(10)
                        .saturating_add((byte - b'0') as usize);
                }
                b';' => {
                    if tab.csi_count < tab.csi_params.len() {
                        tab.csi_count += 1;
                    }
                }
                b'@' | b'A' | b'B' | b'C' | b'D' | b'G' | b'H' | b'J' | b'K' | b'L' | b'M'
                | b'P' | b'X' | b'm' | b'h' | b'l' | b's' | b'u' => {
                    apply_csi(tab, columns, tab_index, byte);
                    reset_escape(tab);
                }
                _ => reset_escape(tab),
            },
            ParseState::Osc => match byte {
                b'\x07' => finish_osc(tab),
                b'\x1b' => tab.osc_esc_pending = true,
                b'\\' if tab.osc_esc_pending => finish_osc(tab),
                _ => {
                    if tab.osc_esc_pending && tab.osc_len < tab.osc_bytes.len() {
                        tab.osc_bytes[tab.osc_len] = b'\x1b';
                        tab.osc_len += 1;
                    }
                    tab.osc_esc_pending = false;
                    if tab.osc_len < tab.osc_bytes.len() {
                        tab.osc_bytes[tab.osc_len] = byte;
                        tab.osc_len += 1;
                    }
                }
            },
        }
    }
}

fn reset_escape(tab: &mut TerminalTab) {
    tab.parse_state = ParseState::Ground;
    tab.csi_count = 0;
    tab.csi_private = false;
}

fn finish_osc(tab: &mut TerminalTab) {
    if let Some(separator) = tab.osc_bytes[..tab.osc_len]
        .iter()
        .position(|byte| *byte == b';')
    {
        if &tab.osc_bytes[..separator] == b"0" || &tab.osc_bytes[..separator] == b"2" {
            let title = &tab.osc_bytes[separator + 1..tab.osc_len];
            let title_len = title.len().min(tab.title.len());
            tab.title[..title_len].copy_from_slice(&title[..title_len]);
            tab.title_len = title_len;
        }
    }
    reset_escape(tab);
    tab.osc_len = 0;
    tab.osc_esc_pending = false;
}

fn reset_terminal_tab(tab: &mut TerminalTab, tab_index: usize) {
    let session_handle = tab.session_handle;
    let session_id = tab.session_id;
    *tab = TerminalTab::empty();
    tab.occupied = true;
    tab.session_handle = session_handle;
    tab.session_id = session_id;
    tab.title[..4].copy_from_slice(b"SHELL");
    tab.title_len = 5;
    tab.title[4] = b'0' + tab_index as u8;
    crate::tabs::clear_tab_grid(tab_index);
}

fn apply_csi(tab: &mut TerminalTab, columns: usize, tab_index: usize, opcode: u8) {
    match opcode {
        b'A' => tab.cursor_line = tab.cursor_line.saturating_sub(csi_param(tab, 0, 1)),
        b'B' => {
            tab.cursor_line =
                (tab.cursor_line + csi_param(tab, 0, 1)).min(tab.line_count.saturating_sub(1))
        }
        b'C' => {
            tab.cursor_col = (tab.cursor_col + csi_param(tab, 0, 1)).min(columns.saturating_sub(1))
        }
        b'D' => tab.cursor_col = tab.cursor_col.saturating_sub(csi_param(tab, 0, 1)),
        b'G' => {
            tab.cursor_col = csi_param(tab, 0, 1)
                .saturating_sub(1)
                .min(columns.saturating_sub(1))
        }
        b'H' => {
            tab.cursor_line = csi_param(tab, 0, 1)
                .saturating_sub(1)
                .min(MAX_SCROLLBACK_LINES.saturating_sub(1));
            tab.cursor_col = csi_param(tab, 1, 1)
                .saturating_sub(1)
                .min(columns.saturating_sub(1));
        }
        b'J' => clear_display(tab, tab_index, columns, csi_param(tab, 0, 0)),
        b'K' => clear_line_mode(tab, columns, tab_index, csi_param(tab, 0, 0)),
        b'L' => insert_lines(tab, tab_index, csi_param(tab, 0, 1)),
        b'M' => delete_lines(tab, tab_index, csi_param(tab, 0, 1)),
        b'P' => delete_chars(tab, columns, tab_index, csi_param(tab, 0, 1)),
        b'X' => erase_chars(tab, columns, tab_index, csi_param(tab, 0, 1)),
        b'@' => insert_blank_chars(tab, columns, tab_index, csi_param(tab, 0, 1)),
        b'm' => apply_sgr(tab),
        b'h' if tab.csi_private && csi_param(tab, 0, 0) == 25 => tab.cursor_visible = true,
        b'l' if tab.csi_private && csi_param(tab, 0, 0) == 25 => tab.cursor_visible = false,
        b's' => {
            tab.saved_cursor_line = tab.cursor_line;
            tab.saved_cursor_col = tab.cursor_col;
        }
        b'u' => {
            tab.cursor_line = tab.saved_cursor_line.min(tab.line_count.saturating_sub(1));
            tab.cursor_col = tab.saved_cursor_col.min(columns.saturating_sub(1));
        }
        _ => {}
    }
}

fn apply_sgr(tab: &mut TerminalTab) {
    if tab.csi_count == 0 {
        tab.current_fg = COLOR_DEFAULT;
        tab.current_bg = COLOR_DEFAULT;
        tab.current_flags = 0;
        return;
    }
    for index in 0..tab.csi_count {
        match tab.csi_params[index] {
            0 => {
                tab.current_fg = COLOR_DEFAULT;
                tab.current_bg = COLOR_DEFAULT;
                tab.current_flags = 0;
            }
            1 => tab.current_flags |= CELL_FLAG_BOLD,
            7 => tab.current_flags |= CELL_FLAG_INVERSE,
            22 => tab.current_flags &= !CELL_FLAG_BOLD,
            27 => tab.current_flags &= !CELL_FLAG_INVERSE,
            30..=37 => tab.current_fg = (tab.csi_params[index] - 30 + 1) as u8,
            39 => tab.current_fg = COLOR_DEFAULT,
            40..=47 => tab.current_bg = (tab.csi_params[index] - 40 + 1) as u8,
            49 => tab.current_bg = COLOR_DEFAULT,
            90..=97 => tab.current_fg = (tab.csi_params[index] - 90 + 9) as u8,
            100..=107 => tab.current_bg = (tab.csi_params[index] - 100 + 9) as u8,
            _ => {}
        }
    }
}

fn csi_param(tab: &TerminalTab, index: usize, default: usize) -> usize {
    tab.csi_params
        .get(index)
        .copied()
        .unwrap_or(default)
        .max(default)
}

fn put_char(tab: &mut TerminalTab, columns: usize, tab_index: usize, byte: u8) {
    if tab.cursor_col >= columns {
        wrap_to_next_line(tab, tab_index);
    }
    unsafe {
        let grid = GRIDS.tab_mut(tab_index);
        grid[tab.cursor_line][tab.cursor_col] = Cell {
            ch: byte,
            fg: tab.current_fg,
            bg: tab.current_bg,
            flags: tab.current_flags,
        };
    }
    tab.cursor_col += 1;
    if tab.cursor_col >= columns {
        wrap_to_next_line(tab, tab_index);
    }
}

fn advance_tab_stop(tab: &mut TerminalTab, columns: usize, tab_index: usize) {
    let next = ((tab.cursor_col / 8) + 1) * 8;
    while tab.cursor_col < next.min(columns) {
        put_char(tab, columns, tab_index, b' ');
    }
}

fn explicit_new_line(tab: &mut TerminalTab, tab_index: usize) {
    tab.cursor_col = 0;
    advance_line(tab, tab_index, false);
}

fn wrap_to_next_line(tab: &mut TerminalTab, tab_index: usize) {
    advance_line(tab, tab_index, true);
}

fn advance_line(tab: &mut TerminalTab, tab_index: usize, wrapped: bool) {
    unsafe {
        WRAPS.tab_mut(tab_index)[tab.cursor_line] = wrapped;
    }
    if tab.cursor_line + 1 >= MAX_SCROLLBACK_LINES {
        scroll_grid(tab_index, 1);
        tab.cursor_line = MAX_SCROLLBACK_LINES - 1;
    } else {
        tab.cursor_line += 1;
        tab.line_count = tab.line_count.max(tab.cursor_line + 1);
    }
    tab.cursor_col = 0;
}

fn clear_display(tab: &mut TerminalTab, tab_index: usize, columns: usize, mode: usize) {
    unsafe {
        let grid = GRIDS.tab_mut(tab_index);
        match mode {
            0 => {
                for col in tab.cursor_col..columns {
                    grid[tab.cursor_line][col] = Cell::blank();
                }
                for line in tab.cursor_line + 1..tab.line_count {
                    grid[line][..columns].fill(Cell::blank());
                }
            }
            1 => {
                for line in 0..tab.cursor_line {
                    grid[line][..columns].fill(Cell::blank());
                }
                for col in 0..=tab.cursor_col.min(columns.saturating_sub(1)) {
                    grid[tab.cursor_line][col] = Cell::blank();
                }
            }
            2 => {
                for line in 0..tab.line_count {
                    grid[line][..columns].fill(Cell::blank());
                }
                tab.cursor_line = 0;
                tab.cursor_col = 0;
            }
            _ => {}
        }
    }
}

fn clear_line_mode(tab: &mut TerminalTab, columns: usize, tab_index: usize, mode: usize) {
    unsafe {
        let grid = GRIDS.tab_mut(tab_index);
        match mode {
            0 => {
                for col in tab.cursor_col..columns {
                    grid[tab.cursor_line][col] = Cell::blank();
                }
            }
            1 => {
                for col in 0..=tab.cursor_col.min(columns.saturating_sub(1)) {
                    grid[tab.cursor_line][col] = Cell::blank();
                }
            }
            2 => grid[tab.cursor_line][..columns].fill(Cell::blank()),
            _ => {}
        }
    }
}

fn erase_chars(tab: &mut TerminalTab, columns: usize, tab_index: usize, count: usize) {
    unsafe {
        let grid = GRIDS.tab_mut(tab_index);
        for col in tab.cursor_col..(tab.cursor_col + count).min(columns) {
            grid[tab.cursor_line][col] = Cell::blank();
        }
    }
}

fn delete_chars(tab: &mut TerminalTab, columns: usize, tab_index: usize, count: usize) {
    unsafe {
        let grid = GRIDS.tab_mut(tab_index);
        let row = &mut grid[tab.cursor_line];
        row.copy_within(
            (tab.cursor_col + count).min(columns)..columns,
            tab.cursor_col,
        );
        row[columns.saturating_sub(count.min(columns - tab.cursor_col))..columns]
            .fill(Cell::blank());
    }
}

fn insert_blank_chars(tab: &mut TerminalTab, columns: usize, tab_index: usize, count: usize) {
    unsafe {
        let grid = GRIDS.tab_mut(tab_index);
        let row = &mut grid[tab.cursor_line];
        let shift = count.min(columns.saturating_sub(tab.cursor_col));
        if shift == 0 {
            return;
        }
        row.copy_within(tab.cursor_col..columns - shift, tab.cursor_col + shift);
        row[tab.cursor_col..tab.cursor_col + shift].fill(Cell::blank());
    }
}

fn insert_lines(tab: &mut TerminalTab, tab_index: usize, count: usize) {
    unsafe {
        let grid = GRIDS.tab_mut(tab_index);
        let wraps = WRAPS.tab_mut(tab_index);
        let shift = count.min(MAX_SCROLLBACK_LINES.saturating_sub(tab.cursor_line));
        if shift == 0 {
            return;
        }
        grid.copy_within(
            tab.cursor_line..MAX_SCROLLBACK_LINES - shift,
            tab.cursor_line + shift,
        );
        wraps.copy_within(
            tab.cursor_line..MAX_SCROLLBACK_LINES - shift,
            tab.cursor_line + shift,
        );
        for line in tab.cursor_line..tab.cursor_line + shift {
            grid[line].fill(Cell::blank());
            wraps[line] = false;
        }
    }
}

fn delete_lines(tab: &mut TerminalTab, tab_index: usize, count: usize) {
    unsafe {
        let grid = GRIDS.tab_mut(tab_index);
        let wraps = WRAPS.tab_mut(tab_index);
        let shift = count.min(MAX_SCROLLBACK_LINES.saturating_sub(tab.cursor_line));
        if shift == 0 {
            return;
        }
        grid.copy_within(
            tab.cursor_line + shift..MAX_SCROLLBACK_LINES,
            tab.cursor_line,
        );
        wraps.copy_within(
            tab.cursor_line + shift..MAX_SCROLLBACK_LINES,
            tab.cursor_line,
        );
        for line in MAX_SCROLLBACK_LINES - shift..MAX_SCROLLBACK_LINES {
            grid[line].fill(Cell::blank());
            wraps[line] = false;
        }
    }
}

pub(crate) fn scroll_grid(tab_index: usize, lines: usize) {
    unsafe {
        let grid = GRIDS.tab_mut(tab_index);
        let wraps = WRAPS.tab_mut(tab_index);
        grid.copy_within(lines..MAX_SCROLLBACK_LINES, 0);
        wraps.copy_within(lines..MAX_SCROLLBACK_LINES, 0);
        for line in MAX_SCROLLBACK_LINES - lines..MAX_SCROLLBACK_LINES {
            grid[line].fill(Cell::blank());
            wraps[line] = false;
        }
    }
}

pub(crate) fn reflow_tab(
    tab: &mut TerminalTab,
    tab_index: usize,
    old_columns: usize,
    new_columns: usize,
) {
    if old_columns == 0 || new_columns == 0 || old_columns == new_columns {
        return;
    }
    unsafe {
        let source = GRIDS.tab(tab_index);
        let source_wraps = WRAPS.tab_mut(tab_index);
        let target = REFLOW_CELLS.get();
        let target_wraps = REFLOW_WRAPS.get();
        target.fill([Cell::blank(); MAX_COLS]);
        target_wraps.fill(false);

        let mut out_line = 0usize;
        let mut out_col = 0usize;
        let mut row = 0usize;
        while row < tab.line_count.min(MAX_SCROLLBACK_LINES) && out_line < MAX_SCROLLBACK_LINES {
            let mut visual_len = row_visual_len(&source[row], old_columns);
            if source_wraps[row] && visual_len == old_columns {
                visual_len = old_columns;
            }
            for col in 0..visual_len.min(old_columns) {
                if out_col == new_columns {
                    target_wraps[out_line] = true;
                    out_line += 1;
                    out_col = 0;
                    if out_line >= MAX_SCROLLBACK_LINES {
                        break;
                    }
                }
                target[out_line][out_col] = source[row][col];
                out_col += 1;
            }
            if out_line >= MAX_SCROLLBACK_LINES {
                break;
            }
            if !source_wraps[row] {
                out_line += 1;
                out_col = 0;
            }
            row += 1;
        }
        let new_line_count = out_line.max(1).min(MAX_SCROLLBACK_LINES);
        let dest = GRIDS.tab_mut(tab_index);
        dest.copy_from_slice(target);
        source_wraps.copy_from_slice(target_wraps);
        tab.line_count = new_line_count;
        tab.cursor_line = tab.cursor_line.min(tab.line_count.saturating_sub(1));
        tab.cursor_col = tab.cursor_col.min(new_columns.saturating_sub(1));
    }
}

pub(crate) fn row_visual_len(row: &[Cell; MAX_COLS], columns: usize) -> usize {
    row[..columns]
        .iter()
        .rposition(|cell| *cell != Cell::blank())
        .map(|index| index + 1)
        .unwrap_or(0)
}
