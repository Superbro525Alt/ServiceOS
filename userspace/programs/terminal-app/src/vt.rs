use super::*;

pub(crate) fn apply_output(pane: &mut TerminalPane, slot: usize, bytes: &[u8]) {
    let columns = pane.columns;
    pane.scroll_offset = 0;
    for byte in bytes.iter().copied() {
        match pane.parse_state {
            ParseState::Ground => match byte {
                b'\x1b' => pane.parse_state = ParseState::Esc,
                b'\r' => pane.cursor_col = 0,
                b'\n' => explicit_new_line(pane, slot),
                b'\t' => advance_tab_stop(pane, columns, slot),
                0x20..=0x7e => put_char(pane, columns, slot, byte),
                _ => {}
            },
            ParseState::Esc => match byte {
                b'[' => {
                    pane.parse_state = ParseState::Csi;
                    pane.csi_params = [0; 8];
                    pane.csi_count = 1;
                    pane.csi_private = false;
                }
                b']' => {
                    pane.parse_state = ParseState::Osc;
                    pane.osc_len = 0;
                    pane.osc_esc_pending = false;
                }
                b'7' => {
                    pane.saved_cursor_line = pane.cursor_line;
                    pane.saved_cursor_col = pane.cursor_col;
                    reset_escape(pane);
                }
                b'8' => {
                    pane.cursor_line = pane.cursor_line.min(pane.line_count.saturating_sub(1));
                    pane.cursor_col = pane.saved_cursor_col.min(columns.saturating_sub(1));
                    reset_escape(pane);
                }
                b'c' => {
                    reset_terminal_pane(pane, slot);
                    reset_escape(pane);
                }
                _ => reset_escape(pane),
            },
            ParseState::Csi => match byte {
                b'?' if pane.csi_count == 1 && pane.csi_params[0] == 0 => pane.csi_private = true,
                b'0'..=b'9' => {
                    let index = pane
                        .csi_count
                        .saturating_sub(1)
                        .min(pane.csi_params.len() - 1);
                    pane.csi_params[index] = pane.csi_params[index]
                        .saturating_mul(10)
                        .saturating_add((byte - b'0') as usize);
                }
                b';' => {
                    if pane.csi_count < pane.csi_params.len() {
                        pane.csi_count += 1;
                    }
                }
                b'@' | b'A' | b'B' | b'C' | b'D' | b'G' | b'H' | b'J' | b'K' | b'L' | b'M'
                | b'P' | b'X' | b'm' | b'h' | b'l' | b's' | b'u' => {
                    apply_csi(pane, columns, slot, byte);
                    reset_escape(pane);
                }
                _ => reset_escape(pane),
            },
            ParseState::Osc => match byte {
                b'\x07' => finish_osc(pane),
                b'\x1b' => pane.osc_esc_pending = true,
                b'\\' if pane.osc_esc_pending => finish_osc(pane),
                _ => {
                    if pane.osc_esc_pending && pane.osc_len < pane.osc_bytes.len() {
                        pane.osc_bytes[pane.osc_len] = b'\x1b';
                        pane.osc_len += 1;
                    }
                    pane.osc_esc_pending = false;
                    if pane.osc_len < pane.osc_bytes.len() {
                        pane.osc_bytes[pane.osc_len] = byte;
                        pane.osc_len += 1;
                    }
                }
            },
        }
    }
}

fn reset_escape(pane: &mut TerminalPane) {
    pane.parse_state = ParseState::Ground;
    pane.csi_count = 0;
    pane.csi_private = false;
}

fn finish_osc(pane: &mut TerminalPane) {
    if let Some(separator) = pane.osc_bytes[..pane.osc_len]
        .iter()
        .position(|byte| *byte == b';')
    {
        if &pane.osc_bytes[..separator] == b"0" || &pane.osc_bytes[..separator] == b"2" {
            let title = &pane.osc_bytes[separator + 1..pane.osc_len];
            let title_len = title.len().min(pane.title.len());
            pane.title[..title_len].copy_from_slice(&title[..title_len]);
            pane.title_len = title_len;
        }
    }
    reset_escape(pane);
    pane.osc_len = 0;
    pane.osc_esc_pending = false;
}

fn reset_terminal_pane(pane: &mut TerminalPane, slot: usize) {
    let session_handle = pane.session_handle;
    let session_id = pane.session_id;
    let columns = pane.columns;
    let rows = pane.rows;
    *pane = TerminalPane::empty();
    pane.session_handle = session_handle;
    pane.session_id = session_id;
    pane.columns = columns;
    pane.rows = rows;
    let label_len = 5 + 1;
    pane.title[..label_len].copy_from_slice(b"SHELL0");
    pane.title[5] = b'1' + (slot % MAX_PANES_PER_TAB) as u8;
    pane.title_len = label_len;
    crate::panes::clear_pane_grid(slot);
}

fn apply_csi(pane: &mut TerminalPane, columns: usize, slot: usize, opcode: u8) {
    match opcode {
        b'A' => pane.cursor_line = pane.cursor_line.saturating_sub(csi_param(pane, 0, 1)),
        b'B' => {
            pane.cursor_line =
                (pane.cursor_line + csi_param(pane, 0, 1)).min(pane.line_count.saturating_sub(1))
        }
        b'C' => {
            pane.cursor_col =
                (pane.cursor_col + csi_param(pane, 0, 1)).min(columns.saturating_sub(1))
        }
        b'D' => pane.cursor_col = pane.cursor_col.saturating_sub(csi_param(pane, 0, 1)),
        b'G' => {
            pane.cursor_col = csi_param(pane, 0, 1)
                .saturating_sub(1)
                .min(columns.saturating_sub(1))
        }
        b'H' => {
            pane.cursor_line = csi_param(pane, 0, 1)
                .saturating_sub(1)
                .min(MAX_SCROLLBACK_LINES.saturating_sub(1));
            pane.cursor_col = csi_param(pane, 1, 1)
                .saturating_sub(1)
                .min(columns.saturating_sub(1));
        }
        b'J' => clear_display(pane, slot, columns, csi_param(pane, 0, 0)),
        b'K' => clear_line_mode(pane, columns, slot, csi_param(pane, 0, 0)),
        b'L' => insert_lines(pane, slot, csi_param(pane, 0, 1)),
        b'M' => delete_lines(pane, slot, csi_param(pane, 0, 1)),
        b'P' => delete_chars(pane, columns, slot, csi_param(pane, 0, 1)),
        b'X' => erase_chars(pane, columns, slot, csi_param(pane, 0, 1)),
        b'@' => insert_blank_chars(pane, columns, slot, csi_param(pane, 0, 1)),
        b'm' => apply_sgr(pane),
        b'h' if pane.csi_private && csi_param(pane, 0, 0) == 25 => pane.cursor_visible = true,
        b'l' if pane.csi_private && csi_param(pane, 0, 0) == 25 => pane.cursor_visible = false,
        b's' => {
            pane.saved_cursor_line = pane.cursor_line;
            pane.saved_cursor_col = pane.cursor_col;
        }
        b'u' => {
            pane.cursor_line = pane.saved_cursor_line.min(pane.line_count.saturating_sub(1));
            pane.cursor_col = pane.saved_cursor_col.min(columns.saturating_sub(1));
        }
        _ => {}
    }
}

fn apply_sgr(pane: &mut TerminalPane) {
    if pane.csi_count == 0 {
        pane.current_fg = COLOR_DEFAULT;
        pane.current_bg = COLOR_DEFAULT;
        pane.current_flags = 0;
        return;
    }
    for index in 0..pane.csi_count {
        match pane.csi_params[index] {
            0 => {
                pane.current_fg = COLOR_DEFAULT;
                pane.current_bg = COLOR_DEFAULT;
                pane.current_flags = 0;
            }
            1 => pane.current_flags |= CELL_FLAG_BOLD,
            7 => pane.current_flags |= CELL_FLAG_INVERSE,
            22 => pane.current_flags &= !CELL_FLAG_BOLD,
            27 => pane.current_flags &= !CELL_FLAG_INVERSE,
            30..=37 => pane.current_fg = (pane.csi_params[index] - 30 + 1) as u8,
            39 => pane.current_fg = COLOR_DEFAULT,
            40..=47 => pane.current_bg = (pane.csi_params[index] - 40 + 1) as u8,
            49 => pane.current_bg = COLOR_DEFAULT,
            90..=97 => pane.current_fg = (pane.csi_params[index] - 90 + 9) as u8,
            100..=107 => pane.current_bg = (pane.csi_params[index] - 100 + 9) as u8,
            _ => {}
        }
    }
}

fn csi_param(pane: &TerminalPane, index: usize, default: usize) -> usize {
    pane.csi_params
        .get(index)
        .copied()
        .unwrap_or(default)
        .max(default)
}

fn put_char(pane: &mut TerminalPane, columns: usize, slot: usize, byte: u8) {
    if columns == 0 {
        return;
    }
    if pane.cursor_col >= columns {
        wrap_to_next_line(pane, slot);
    }
    unsafe {
        let grid = GRIDS.pane_mut(slot);
        grid[pane.cursor_line][pane.cursor_col] = Cell {
            ch: byte,
            fg: pane.current_fg,
            bg: pane.current_bg,
            flags: pane.current_flags,
        };
    }
    pane.cursor_col += 1;
    if pane.cursor_col >= columns {
        wrap_to_next_line(pane, slot);
    }
}

fn advance_tab_stop(pane: &mut TerminalPane, columns: usize, slot: usize) {
    let next = ((pane.cursor_col / 8) + 1) * 8;
    while pane.cursor_col < next.min(columns) {
        put_char(pane, columns, slot, b' ');
    }
}

fn explicit_new_line(pane: &mut TerminalPane, slot: usize) {
    pane.cursor_col = 0;
    advance_line(pane, slot, false);
}

fn wrap_to_next_line(pane: &mut TerminalPane, slot: usize) {
    advance_line(pane, slot, true);
}

fn advance_line(pane: &mut TerminalPane, slot: usize, wrapped: bool) {
    unsafe {
        WRAPS.wraps_mut(slot)[pane.cursor_line] = wrapped;
    }
    if pane.cursor_line + 1 >= MAX_SCROLLBACK_LINES {
        scroll_pane_grid(slot, 1);
        pane.cursor_line = MAX_SCROLLBACK_LINES - 1;
    } else {
        pane.cursor_line += 1;
        pane.line_count = pane.line_count.max(pane.cursor_line + 1);
    }
    pane.cursor_col = 0;
}

fn clear_display(pane: &mut TerminalPane, slot: usize, columns: usize, mode: usize) {
    unsafe {
        let grid = GRIDS.pane_mut(slot);
        match mode {
            0 => {
                for col in pane.cursor_col..columns {
                    grid[pane.cursor_line][col] = Cell::blank();
                }
                for line in pane.cursor_line + 1..pane.line_count {
                    grid[line][..columns].fill(Cell::blank());
                }
            }
            1 => {
                for line in 0..pane.cursor_line {
                    grid[line][..columns].fill(Cell::blank());
                }
                for col in 0..=pane.cursor_col.min(columns.saturating_sub(1)) {
                    grid[pane.cursor_line][col] = Cell::blank();
                }
            }
            2 => {
                for line in 0..pane.line_count {
                    grid[line][..columns].fill(Cell::blank());
                }
                pane.cursor_line = 0;
                pane.cursor_col = 0;
            }
            _ => {}
        }
    }
}

fn clear_line_mode(pane: &mut TerminalPane, columns: usize, slot: usize, mode: usize) {
    unsafe {
        let grid = GRIDS.pane_mut(slot);
        match mode {
            0 => {
                for col in pane.cursor_col..columns {
                    grid[pane.cursor_line][col] = Cell::blank();
                }
            }
            1 => {
                for col in 0..=pane.cursor_col.min(columns.saturating_sub(1)) {
                    grid[pane.cursor_line][col] = Cell::blank();
                }
            }
            2 => grid[pane.cursor_line][..columns].fill(Cell::blank()),
            _ => {}
        }
    }
}

fn erase_chars(pane: &mut TerminalPane, columns: usize, slot: usize, count: usize) {
    unsafe {
        let grid = GRIDS.pane_mut(slot);
        for col in pane.cursor_col..(pane.cursor_col + count).min(columns) {
            grid[pane.cursor_line][col] = Cell::blank();
        }
    }
}

fn delete_chars(pane: &mut TerminalPane, columns: usize, slot: usize, count: usize) {
    unsafe {
        let grid = GRIDS.pane_mut(slot);
        let row = &mut grid[pane.cursor_line];
        row.copy_within(
            (pane.cursor_col + count).min(columns)..columns,
            pane.cursor_col,
        );
        row[columns.saturating_sub(count.min(columns - pane.cursor_col))..columns]
            .fill(Cell::blank());
    }
}

fn insert_blank_chars(pane: &mut TerminalPane, columns: usize, slot: usize, count: usize) {
    unsafe {
        let grid = GRIDS.pane_mut(slot);
        let row = &mut grid[pane.cursor_line];
        let shift = count.min(columns.saturating_sub(pane.cursor_col));
        if shift == 0 {
            return;
        }
        row.copy_within(pane.cursor_col..columns - shift, pane.cursor_col + shift);
        row[pane.cursor_col..pane.cursor_col + shift].fill(Cell::blank());
    }
}

fn insert_lines(pane: &mut TerminalPane, slot: usize, count: usize) {
    unsafe {
        let grid = GRIDS.pane_mut(slot);
        let wraps = WRAPS.wraps_mut(slot);
        let shift = count.min(MAX_SCROLLBACK_LINES.saturating_sub(pane.cursor_line));
        if shift == 0 {
            return;
        }
        grid.copy_within(
            pane.cursor_line..MAX_SCROLLBACK_LINES - shift,
            pane.cursor_line + shift,
        );
        wraps.copy_within(
            pane.cursor_line..MAX_SCROLLBACK_LINES - shift,
            pane.cursor_line + shift,
        );
        for line in pane.cursor_line..pane.cursor_line + shift {
            grid[line].fill(Cell::blank());
            wraps[line] = false;
        }
    }
}

fn delete_lines(pane: &mut TerminalPane, slot: usize, count: usize) {
    unsafe {
        let grid = GRIDS.pane_mut(slot);
        let wraps = WRAPS.wraps_mut(slot);
        let shift = count.min(MAX_SCROLLBACK_LINES.saturating_sub(pane.cursor_line));
        if shift == 0 {
            return;
        }
        grid.copy_within(
            pane.cursor_line + shift..MAX_SCROLLBACK_LINES,
            pane.cursor_line,
        );
        wraps.copy_within(
            pane.cursor_line + shift..MAX_SCROLLBACK_LINES,
            pane.cursor_line,
        );
        for line in MAX_SCROLLBACK_LINES - shift..MAX_SCROLLBACK_LINES {
            grid[line].fill(Cell::blank());
            wraps[line] = false;
        }
    }
}

pub(crate) fn scroll_pane_grid(slot: usize, lines: usize) {
    unsafe {
        let grid = GRIDS.pane_mut(slot);
        let wraps = WRAPS.wraps_mut(slot);
        grid.copy_within(lines..MAX_SCROLLBACK_LINES, 0);
        wraps.copy_within(lines..MAX_SCROLLBACK_LINES, 0);
        for line in MAX_SCROLLBACK_LINES - lines..MAX_SCROLLBACK_LINES {
            grid[line].fill(Cell::blank());
            wraps[line] = false;
        }
    }
}

pub(crate) fn reflow_pane(
    pane: &mut TerminalPane,
    slot: usize,
    old_columns: usize,
    new_columns: usize,
) {
    if old_columns == 0 || new_columns == 0 || old_columns == new_columns {
        return;
    }
    unsafe {
        let source = GRIDS.pane(slot);
        let source_wraps = WRAPS.wraps_mut(slot);
        let target = REFLOW_CELLS.get();
        let target_wraps = REFLOW_WRAPS.get();
        target.fill([Cell::blank(); MAX_COLS]);
        target_wraps.fill(false);

        let mut out_line = 0usize;
        let mut out_col = 0usize;
        let mut row = 0usize;
        while row < pane.line_count.min(MAX_SCROLLBACK_LINES) && out_line < MAX_SCROLLBACK_LINES {
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
        let dest = GRIDS.pane_mut(slot);
        dest.copy_from_slice(target);
        source_wraps.copy_from_slice(target_wraps);
        pane.line_count = new_line_count;
        pane.cursor_line = pane.cursor_line.min(pane.line_count.saturating_sub(1));
        pane.cursor_col = pane.cursor_col.min(new_columns.saturating_sub(1));
    }
}

pub(crate) fn row_visual_len(row: &[Cell; MAX_COLS], columns: usize) -> usize {
    row[..columns]
        .iter()
        .rposition(|cell| *cell != Cell::blank())
        .map(|index| index + 1)
        .unwrap_or(0)
}
