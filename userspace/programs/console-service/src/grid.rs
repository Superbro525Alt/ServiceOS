//! Retained VT text grid for the kernel/service console surface.
//!
//! The console service formats every structured record it renders to the
//! serial console into a fixed 80x24 character grid (newest rows win, ring
//! ordered). Subscribed console-session clients can pull the surface as a
//! VT-style escape stream (`ESC[2J ESC[H` plus rows joined with CRLF), which
//! any ANSI text renderer can draw without new contract tags.

use core::cell::UnsafeCell;

pub(crate) const GRID_COLS: usize = 80;
pub(crate) const GRID_ROWS: usize = 24;

/// VT escape prefix sent before every frame: clear screen, home cursor.
pub(crate) const FRAME_RESET: &[u8] = b"\x1b[2J\x1b[H";
/// Alternate-screen style opt-in markers carried inside existing
/// `SessionWriteText` payloads so clients can subscribe without new tags.
pub(crate) const GRID_SUBSCRIBE: &[u8] = b"\x1b[?1049h";
pub(crate) const GRID_UNSUBSCRIBE: &[u8] = b"\x1b[?1049l";

#[derive(Clone, Copy)]
pub(crate) struct TextGrid {
    cells: [u8; GRID_ROWS * GRID_COLS],
    /// Ring cursor: index of the oldest retained row.
    head: usize,
    filled: usize,
}

impl TextGrid {
    pub(crate) const fn new() -> Self {
        Self {
            cells: [b' '; GRID_ROWS * GRID_COLS],
            head: 0,
            filled: 0,
        }
    }

    /// Append one rendered console line as the newest row. Only printable
    /// ASCII is stored; longer lines clamp to `GRID_COLS`.
    pub(crate) fn push_line(&mut self, line: &[u8]) {
        let row = if self.filled < GRID_ROWS {
            self.filled
        } else {
            self.head
        };
        let base = row * GRID_COLS;
        let mut col = 0usize;
        for &byte in line {
            if col >= GRID_COLS {
                break;
            }
            if (0x20..=0x7e).contains(&byte) {
                self.cells[base + col] = byte;
                col += 1;
            }
        }
        while col < GRID_COLS {
            self.cells[base + col] = b' ';
            col += 1;
        }
        if self.filled < GRID_ROWS {
            self.filled += 1;
        } else {
            self.head = (self.head + 1) % GRID_ROWS;
        }
    }

    /// Encode the full surface as a VT byte stream into `out`, returning the
    /// encoded length. Rows are emitted oldest-first and padded to `GRID_COLS`.
    pub(crate) fn frame(&self, out: &mut [u8]) -> usize {
        let required = FRAME_RESET.len() + GRID_ROWS * (GRID_COLS + 2);
        if out.len() < required {
            return 0;
        }
        let mut len = 0usize;
        len += copy_into(FRAME_RESET, out, len);
        for row in 0..GRID_ROWS {
            let base = ((self.head + row) % GRID_ROWS) * GRID_COLS;
            len += copy_into(&self.cells[base..base + GRID_COLS], out, len);
            len += copy_into(b"\r\n", out, len);
        }
        len
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn row(&self, order: usize) -> &[u8] {
        let index = (self.head + order) % GRID_ROWS;
        let base = index * GRID_COLS;
        &self.cells[base..base + GRID_COLS]
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn filled_rows(&self) -> usize {
        self.filled
    }
}

fn copy_into(source: &[u8], out: &mut [u8], offset: usize) -> usize {
    let end = (offset + source.len()).min(out.len());
    let count = end - offset;
    out[offset..end].copy_from_slice(&source[..count]);
    count
}

struct GridSlot(UnsafeCell<TextGrid>);

// SAFETY: the console task is strictly single-threaded; the grid static is
// only touched from that loop (same pattern as shell-service pending lines).
unsafe impl Sync for GridSlot {}

static CONSOLE_GRID: GridSlot = GridSlot(UnsafeCell::new(TextGrid::new()));

/// Record one formatted console line into the retained grid.
pub(crate) fn record_line(line: &[u8]) {
    // SAFETY: single-threaded service; see `GridSlot` note above.
    let grid = unsafe { &mut *CONSOLE_GRID.0.get() };
    grid.push_line(line);
}

/// Snapshot the current surface into `out` (returns encoded length).
pub(crate) fn snapshot_frame(out: &mut [u8]) -> usize {
    // SAFETY: single-threaded service; see `GridSlot` note above.
    let grid = unsafe { &*CONSOLE_GRID.0.get() };
    grid.frame(out)
}

/// True when a payload starts with the subscribe marker.
pub(crate) fn is_subscribe(payload: &[u8]) -> bool {
    payload.len() >= GRID_SUBSCRIBE.len() && &payload[..GRID_SUBSCRIBE.len()] == GRID_SUBSCRIBE
}

/// True when a payload starts with the unsubscribe marker.
pub(crate) fn is_unsubscribe(payload: &[u8]) -> bool {
    payload.len() >= GRID_UNSUBSCRIBE.len()
        && &payload[..GRID_UNSUBSCRIBE.len()] == GRID_UNSUBSCRIBE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_line_clamps_and_pads_to_grid_width() {
        let mut grid = TextGrid::new();
        grid.push_line(b"short");
        assert_eq!(&grid.row(0)[..7], b"short  ");
        let long = [b'x'; GRID_COLS + 10];
        grid.push_line(&long);
        assert_eq!(grid.row(1)[GRID_COLS - 1], b'x');
    }

    #[test]
    fn non_printable_bytes_are_dropped() {
        let mut grid = TextGrid::new();
        grid.push_line(b"a\x00b\xff c");
        assert_eq!(&grid.row(0)[..4], b"ab c");
    }

    #[test]
    fn frame_is_reset_prefix_plus_padded_crlf_rows() {
        let mut grid = TextGrid::new();
        grid.push_line(b"boot ok");
        let mut out = [0u8; FRAME_RESET.len() + GRID_ROWS * (GRID_COLS + 2)];
        let len = grid.frame(&mut out);
        assert_eq!(len, out.len());
        assert_eq!(&out[..FRAME_RESET.len()], FRAME_RESET);
        let row_start = FRAME_RESET.len();
        assert_eq!(&out[row_start..row_start + 7], b"boot ok");
        // Every row ends with CRLF.
        let first_row_end = row_start + GRID_COLS;
        assert_eq!(&out[first_row_end..first_row_end + 2], b"\r\n");
        // Unfilled rows stay blank padding.
        let second_row = first_row_end + 2;
        assert!(
            out[second_row..second_row + GRID_COLS]
                .iter()
                .all(|&b| b == b' ')
        );
    }

    #[test]
    fn frame_rejects_undersized_output() {
        let grid = TextGrid::new();
        let mut out = [0u8; 64];
        assert_eq!(grid.frame(&mut out), 0);
    }

    #[test]
    fn rows_wrap_as_a_ring_with_newest_wins() {
        let mut grid = TextGrid::new();
        for index in 0..(GRID_ROWS + 3) as u32 {
            let mut line = [0u8; 4];
            line[0] = b'0' + ((index / 10) % 10) as u8;
            line[1] = b'0' + (index % 10) as u8;
            grid.push_line(&line);
        }
        // Oldest retained row is #3 (indices 0..=2 were overwritten).
        assert_eq!(&grid.row(0)[..2], b"03");
        assert_eq!(
            &grid.row(GRID_ROWS - 1)[..2],
            b"26",
            "newest row is last in frame order"
        );
        assert_eq!(grid.filled_rows(), GRID_ROWS);
    }

    #[test]
    fn markers_match_only_at_payload_start() {
        assert!(is_subscribe(b"\x1b[?1049h"));
        assert!(is_subscribe(b"\x1b[?1049h trailing"));
        assert!(!is_subscribe(b"\x1b[?1049"));
        assert!(!is_subscribe(b"hello"));
        assert!(is_unsubscribe(b"\x1b[?1049l"));
        assert!(!is_unsubscribe(b"\x1b[?1049h"));
    }
}
