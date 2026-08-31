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
            pane.cursor_line = pane
                .saved_cursor_line
                .min(pane.line_count.saturating_sub(1));
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

/// Lines dropped from the top of the reflow staging buffer per overflow
/// shift. Batched so a worst-case wide-to-narrow reflow (256 source lines
/// exploding past the ring capacity) stays a handful of memcpys instead of
/// one shift per produced line.
const REFLOW_DROP_CHUNK: usize = MAX_SCROLLBACK_LINES / 4;

/// Tracks one grid position (the live cursor or the DECSC saved cursor)
/// through a reflow pass. The position re-derives exactly: it maps to
/// wherever its source cell lands in the new geometry, or to the row's
/// next-write position when it sat at or past the streamed span.
struct PositionMap {
    source_line: usize,
    source_col: usize,
    mapped: Option<(usize, usize)>,
}

impl PositionMap {
    fn new(line: usize, col: usize) -> Self {
        Self {
            source_line: line,
            source_col: col,
            mapped: None,
        }
    }

    /// Mid-stream checkpoint: the source cell about to be written at
    /// (out_line, out_col) is this position's cell.
    fn note_cell(&mut self, row: usize, col: usize, out_line: usize, out_col: usize) {
        if self.mapped.is_none() && row == self.source_line && col == self.source_col {
            self.mapped = Some((out_line, out_col));
        }
    }

    /// End-of-row checkpoint: the position sat at or past the row's streamed
    /// span, so it maps to the row's next-write slot (wrapping lazily when
    /// the row exactly filled the new width).
    fn note_row_end(&mut self, row: usize, out_line: usize, out_col: usize, new_columns: usize) {
        if self.mapped.is_none() && row == self.source_line {
            let (line, col) = if out_col >= new_columns {
                (out_line + 1, 0)
            } else {
                (out_line, out_col)
            };
            self.mapped = Some((line.min(MAX_SCROLLBACK_LINES - 1), col));
        }
    }

    /// Drop-oldest shifted the staging buffer up; mapped positions follow.
    fn shift_up(&mut self, lines: usize) {
        if let Some((line, col)) = self.mapped {
            self.mapped = Some((line.saturating_sub(lines), col));
        }
    }
}

/// Drop the oldest staged lines when reflow output exceeds the ring,
/// keeping the newest (live) content. Batched by REFLOW_DROP_CHUNK.
fn reflow_drop_oldest(
    target: &mut [[Cell; MAX_COLS]; MAX_SCROLLBACK_LINES],
    target_wraps: &mut [bool; MAX_SCROLLBACK_LINES],
    out_line: &mut usize,
    cursor_map: &mut PositionMap,
    saved_map: &mut PositionMap,
) {
    target.copy_within(REFLOW_DROP_CHUNK..MAX_SCROLLBACK_LINES, 0);
    target_wraps.copy_within(REFLOW_DROP_CHUNK..MAX_SCROLLBACK_LINES, 0);
    for line in MAX_SCROLLBACK_LINES - REFLOW_DROP_CHUNK..MAX_SCROLLBACK_LINES {
        target[line].fill(Cell::blank());
        target_wraps[line] = false;
    }
    *out_line -= REFLOW_DROP_CHUNK;
    cursor_map.shift_up(REFLOW_DROP_CHUNK);
    saved_map.shift_up(REFLOW_DROP_CHUNK);
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

        let line_count = pane.line_count.min(MAX_SCROLLBACK_LINES);
        let mut cursor_map = PositionMap::new(
            pane.cursor_line.min(line_count.saturating_sub(1)),
            pane.cursor_col,
        );
        let mut saved_map = PositionMap::new(
            pane.saved_cursor_line.min(line_count.saturating_sub(1)),
            pane.saved_cursor_col,
        );

        let mut out_line = 0usize;
        let mut out_col = 0usize;
        let mut row = 0usize;
        while row < line_count {
            // Soft-wrapped rows join with their full source width: erased
            // tails are still logical spaces, so continuation column
            // alignment survives the round trip. Hard rows end at their
            // last written cell.
            let span = if source_wraps[row] {
                old_columns
            } else {
                row_visual_len(&source[row], old_columns)
            };
            let mut col = 0usize;
            while col < span.min(old_columns) {
                if out_col == new_columns {
                    target_wraps[out_line] = true;
                    out_line += 1;
                    out_col = 0;
                    if out_line >= MAX_SCROLLBACK_LINES {
                        reflow_drop_oldest(
                            target,
                            target_wraps,
                            &mut out_line,
                            &mut cursor_map,
                            &mut saved_map,
                        );
                    }
                }
                cursor_map.note_cell(row, col, out_line, out_col);
                saved_map.note_cell(row, col, out_line, out_col);
                target[out_line][out_col] = source[row][col];
                out_col += 1;
                col += 1;
            }
            cursor_map.note_row_end(row, out_line, out_col, new_columns);
            saved_map.note_row_end(row, out_line, out_col, new_columns);
            if !source_wraps[row] {
                out_line += 1;
                out_col = 0;
                if out_line >= MAX_SCROLLBACK_LINES && row + 1 < line_count {
                    reflow_drop_oldest(
                        target,
                        target_wraps,
                        &mut out_line,
                        &mut cursor_map,
                        &mut saved_map,
                    );
                }
            }
            row += 1;
        }
        // The last line counts even when the stream ended mid-soft-span.
        let new_line_count = out_line
            .saturating_add(usize::from(out_col > 0))
            .clamp(1, MAX_SCROLLBACK_LINES);
        let dest = GRIDS.pane_mut(slot);
        dest.copy_from_slice(target);
        source_wraps.copy_from_slice(target_wraps);
        pane.line_count = new_line_count;
        if let Some((line, col)) = cursor_map.mapped {
            pane.cursor_line = line.min(new_line_count - 1);
            pane.cursor_col = col.min(new_columns - 1);
        } else {
            pane.cursor_line = pane.cursor_line.min(new_line_count - 1);
            pane.cursor_col = pane.cursor_col.min(new_columns.saturating_sub(1));
        }
        if let Some((line, col)) = saved_map.mapped {
            pane.saved_cursor_line = line.min(new_line_count - 1);
            pane.saved_cursor_col = col.min(new_columns - 1);
        } else {
            pane.saved_cursor_line = pane.saved_cursor_line.min(new_line_count - 1);
            pane.saved_cursor_col = pane.saved_cursor_col.min(new_columns.saturating_sub(1));
        }
    }
}

pub(crate) fn row_visual_len(row: &[Cell; MAX_COLS], columns: usize) -> usize {
    row[..columns]
        .iter()
        .rposition(|cell| *cell != Cell::blank())
        .map(|index| index + 1)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::string::String;
    use std::sync::Mutex;

    /// reflow_pane stages through the shared REFLOW_CELLS/REFLOW_WRAPS
    /// scratch statics, so host tests serialize their reflow passes. Each
    /// test also owns a disjoint grid slot.
    static REFLOW_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn clear_slot(slot: usize) {
        unsafe {
            GRIDS.pane_mut(slot).fill([Cell::blank(); MAX_COLS]);
            WRAPS.wraps_mut(slot).fill(false);
        }
    }

    fn write_row(slot: usize, line: usize, text: &[u8]) {
        let grid = unsafe { GRIDS.pane_mut(slot) };
        for (col, byte) in text.iter().enumerate() {
            grid[line][col] = Cell {
                ch: *byte,
                fg: COLOR_DEFAULT,
                bg: COLOR_DEFAULT,
                flags: 0,
            };
        }
    }

    fn set_wraps(slot: usize, lines: &[usize]) {
        let wraps = unsafe { WRAPS.wraps_mut(slot) };
        for line in lines {
            wraps[*line] = true;
        }
    }

    fn row_text(slot: usize, line: usize, cols: usize) -> String {
        let grid = unsafe { GRIDS.pane(slot) };
        let end = grid[line][..cols]
            .iter()
            .rposition(|cell| *cell != Cell::blank())
            .map_or(0, |index| index + 1);
        String::from_utf8(grid[line][..end].iter().map(|cell| cell.ch).collect()).unwrap()
    }

    fn row_cells(slot: usize, line: usize, cols: usize) -> std::vec::Vec<u8> {
        let grid = unsafe { GRIDS.pane(slot) };
        grid[line][..cols].iter().map(|cell| cell.ch).collect()
    }

    fn pane_with(line_count: usize, cursor: (usize, usize), saved: (usize, usize)) -> TerminalPane {
        let mut pane = TerminalPane::empty();
        pane.line_count = line_count;
        pane.cursor_line = cursor.0;
        pane.cursor_col = cursor.1;
        pane.saved_cursor_line = saved.0;
        pane.saved_cursor_col = saved.1;
        pane
    }

    #[test]
    fn reflow_narrow_to_wide_rejoins_soft_wraps() {
        let _guard = REFLOW_TEST_LOCK.lock().unwrap();
        let slot = 0;
        clear_slot(slot);
        write_row(slot, 0, b"abcd");
        write_row(slot, 1, b"efgh");
        write_row(slot, 2, b"ij");
        set_wraps(slot, &[0, 1]);
        let mut pane = pane_with(3, (2, 2), (0, 0));

        reflow_pane(&mut pane, slot, 4, 8);

        assert_eq!(pane.line_count, 2);
        assert_eq!(row_text(slot, 0, 8), "abcdefgh");
        assert_eq!(row_text(slot, 1, 8), "ij");
        let wraps = unsafe { WRAPS.wraps_mut(slot) };
        assert!(wraps[0]);
        assert!(!wraps[1]);
        // Cursor sat at the end of the hard tail; it follows the text.
        assert_eq!((pane.cursor_line, pane.cursor_col), (1, 2));
    }

    #[test]
    fn reflow_wide_to_narrow_preserves_hard_lines() {
        let _guard = REFLOW_TEST_LOCK.lock().unwrap();
        let slot = 1;
        clear_slot(slot);
        write_row(slot, 0, b"abcd");
        write_row(slot, 1, b"efgh");
        let mut pane = pane_with(2, (1, 4), (0, 0));

        reflow_pane(&mut pane, slot, 8, 3);

        assert_eq!(pane.line_count, 4);
        assert_eq!(row_text(slot, 0, 3), "abc");
        assert_eq!(row_text(slot, 1, 3), "d");
        assert_eq!(row_text(slot, 2, 3), "efg");
        assert_eq!(row_text(slot, 3, 3), "h");
        let wraps = unsafe { WRAPS.wraps_mut(slot) };
        assert!(wraps[0]);
        assert!(!wraps[1]);
        assert!(wraps[2]);
        assert!(!wraps[3]);
        // Cursor sat one past "efgh"; it lands one past the rewrapped "h".
        assert_eq!((pane.cursor_line, pane.cursor_col), (3, 1));
    }

    #[test]
    fn reflow_unbreakable_run_wraps_at_char_boundary() {
        let _guard = REFLOW_TEST_LOCK.lock().unwrap();
        let slot = 2;
        clear_slot(slot);
        write_row(slot, 0, b"AAAAAAAA");
        let mut pane = pane_with(2, (1, 0), (0, 0));

        // Policy: char-granular wrap. No unbreakable runs exist at cell
        // granularity, so nothing is truncated and no marker is needed.
        reflow_pane(&mut pane, slot, 8, 3);

        assert_eq!(pane.line_count, 4);
        assert_eq!(row_text(slot, 0, 3), "AAA");
        assert_eq!(row_text(slot, 1, 3), "AAA");
        assert_eq!(row_text(slot, 2, 3), "AA");
        let total: usize = (0..3).map(|line| row_text(slot, line, 3).len()).sum();
        assert_eq!(total, 8);
    }

    #[test]
    fn reflow_overflow_drops_oldest_keeps_live_tail() {
        let _guard = REFLOW_TEST_LOCK.lock().unwrap();
        let slot = 3;
        clear_slot(slot);
        // 256 hard lines of 120 identical chars each: shrinking to 8 columns
        // produces 15 output lines per source line (3840), overflowing the
        // 256-line ring. Oldest output must drop; the live tail survives.
        for line in 0..MAX_SCROLLBACK_LINES {
            let row = [b'A' + (line % 26) as u8; 120];
            write_row(slot, line, &row);
        }
        let mut pane = pane_with(MAX_SCROLLBACK_LINES, (255, 119), (0, 0));

        reflow_pane(&mut pane, slot, 120, 8);

        assert_eq!(pane.line_count, MAX_SCROLLBACK_LINES);
        // 3584 output lines dropped: the first retained line is the last
        // chunk of source line 238 ('E'), the last is source line 255's
        // final chunk ('V').
        assert_eq!(row_text(slot, 0, 8), "EEEEEEEE");
        assert_eq!(row_text(slot, 255, 8), "VVVVVVVV");
        // Cursor tracked source (255,119) to output line 255, col 7.
        assert_eq!((pane.cursor_line, pane.cursor_col), (255, 7));
    }

    #[test]
    fn reflow_cursor_map_matrix() {
        let _guard = REFLOW_TEST_LOCK.lock().unwrap();
        let slot = 4;

        // (a) cursor mid first line stays put across a join.
        clear_slot(slot);
        write_row(slot, 0, b"abcd");
        write_row(slot, 1, b"ef");
        set_wraps(slot, &[0]);
        let mut pane = pane_with(2, (0, 2), (0, 0));
        reflow_pane(&mut pane, slot, 4, 8);
        assert_eq!((pane.cursor_line, pane.cursor_col), (0, 2));

        // (b) cursor on the continuation row follows the merge.
        clear_slot(slot);
        write_row(slot, 0, b"abcd");
        write_row(slot, 1, b"ef");
        set_wraps(slot, &[0]);
        let mut pane = pane_with(2, (1, 1), (0, 0));
        reflow_pane(&mut pane, slot, 4, 8);
        assert_eq!((pane.cursor_line, pane.cursor_col), (0, 5));

        // (c) cursor past the row's text maps to the next-write position.
        clear_slot(slot);
        write_row(slot, 0, b"abcd");
        write_row(slot, 1, b"ef");
        set_wraps(slot, &[0]);
        let mut pane = pane_with(2, (1, 3), (0, 0));
        reflow_pane(&mut pane, slot, 4, 8);
        assert_eq!((pane.cursor_line, pane.cursor_col), (0, 6));

        // (d) out-of-range cursor (CSI H can overshoot line_count) clamps.
        clear_slot(slot);
        write_row(slot, 0, b"abcd");
        let mut pane = pane_with(1, (90, 40), (0, 0));
        reflow_pane(&mut pane, slot, 8, 4);
        assert_eq!(pane.cursor_line, 0);
        assert!(pane.cursor_col < 4);
    }

    #[test]
    fn reflow_saved_cursor_and_erased_wrap_tail_align() {
        let _guard = REFLOW_TEST_LOCK.lock().unwrap();
        let slot = 5;
        clear_slot(slot);
        // Row 0 soft-wrapped with its tail erased: the blanks are logical
        // spaces, so the continuation must re-split in place rather than
        // sliding "XY" left.
        write_row(slot, 0, b"abcd");
        write_row(slot, 1, b"XY");
        set_wraps(slot, &[0]);
        let mut pane = pane_with(2, (1, 2), (0, 6));

        reflow_pane(&mut pane, slot, 8, 4);

        assert_eq!(pane.line_count, 3);
        assert_eq!(row_text(slot, 0, 4), "abcd");
        assert_eq!(row_cells(slot, 1, 4), vec![b' ', b' ', b' ', b' ']);
        assert_eq!(row_text(slot, 2, 4), "XY");
        let wraps = unsafe { WRAPS.wraps_mut(slot) };
        assert!(wraps[0]);
        assert!(wraps[1]);
        assert!(!wraps[2]);
        // Saved cursor inside the erased tail maps into the blank span.
        assert_eq!((pane.saved_cursor_line, pane.saved_cursor_col), (1, 2));
        // Live cursor followed "XY".
        assert_eq!((pane.cursor_line, pane.cursor_col), (2, 2));
    }

    #[test]
    fn reattach_replay_rewraps_retained_bytes_at_pane_width() {
        let slot = 6;
        clear_slot(slot);
        // Replay path: retained service bytes stream through the VT parser
        // at the attaching pane's current width, re-deriving the grid.
        let mut pane = TerminalPane::empty();
        pane.columns = 5;
        pane.rows = 10;

        apply_output(&mut pane, slot, b"abcdefghij\r\n$ ");

        assert_eq!(pane.line_count, 4);
        assert_eq!(row_text(slot, 0, 5), "abcde");
        assert_eq!(row_text(slot, 1, 5), "fghij");
        assert_eq!(row_text(slot, 2, 5), "");
        // Trailing blank trims from the visual text; the cursor proves the
        // prompt's trailing space was consumed.
        assert_eq!(row_text(slot, 3, 5), "$");
        let wraps = unsafe { WRAPS.wraps_mut(slot) };
        assert!(wraps[0]);
        assert!(wraps[1]);
        assert!(!wraps[2]);
        assert_eq!((pane.cursor_line, pane.cursor_col), (3, 2));
    }
}
