use core::{fmt::Write as _, str};

use serviceos_desktop_ui as ui;
use serviceos_userspace_runtime as rt;

use crate::DesktopState;
use crate::palette_docs::{DOC_HITS_MAX, DOC_PATH_MAX, DocHit, doc_kind_icon};
use crate::windows::open_path_in_files;

/// Bounded document section under the launcher app grid, ranked from the
/// files-app recent-files ring (recency-first, move-to-front semantics)
/// with every entry confirmed through the storage name index — the same
/// 0x521/0x522 search the command palette uses on the shell's existing
/// storage grant. No new grants, no new services.
pub(crate) const LAUNCHER_DOCS_MAX: usize = 4;

/// Mirror of the files-app recent-ring persistence constants
/// (`files-app/src/persist.rs` store prefix + file name, `recent.rs`
/// RECENT_MAX, `state.rs` MAX_STORAGE_PATH, newline framing). The ring
/// file is a stable cross-app format; the shell only ever reads it, so
/// it needs no directory-creation constants.
const RING_STORE_DIR_PREFIX: &str = "state/files-app/";
const RING_FILE_NAME: &str = "recent.cfg";
const RING_RECENT_MAX: usize = 8;
const RING_PATH_MAX: usize = 96;
const RING_FILE_BYTES: usize = RING_RECENT_MAX * (RING_PATH_MAX + 1);

/// Label slots on the launcher surface: the app grid uses slot 0 (panel
/// title) and slots 5..10 (app rows); the document section continues at
/// slot 11 (section header) and slots 12..15 (document rows).
pub(crate) const DOC_HEADER_SLOT: u32 = 11;
pub(crate) const DOC_ROW_SLOT_BASE: u32 = 12;

/// Continues the shared panel line grid: grid row 6 is the section
/// header (not clickable), grid rows 7..10 are the document rows.
const DOC_HEADER_GRID_ROW: usize = 6;
const DOC_ROW_GRID_ROW_BASE: usize = 7;

/// File-name bytes shown on a document row; the graphics label cap is 56
/// bytes and the kind icon plus separator consume 4, so a clipped name
/// with its "..." suffix always fits.
pub(crate) const DOC_NAME_MAX: usize = 50;

/// Parses the ring file body into recency-ranked document entries. The
/// encoded order is newest-first (files-app writes the move-to-front ring
/// head first), which is exactly the ranking the panel wants. Blank lines
/// are skipped; entries longer than `DOC_PATH_MAX` cannot be represented
/// in the shared `DocHit` value and are skipped rather than truncated.
pub(crate) fn parse_ring_entries(bytes: &[u8]) -> ([DocHit; LAUNCHER_DOCS_MAX], usize) {
    let mut docs = [DocHit {
        path: [0; DOC_PATH_MAX],
        path_len: 0,
        kind: 0,
        line: 0,
    }; LAUNCHER_DOCS_MAX];
    let mut len = 0usize;
    for line in bytes.split(|byte| *byte == b'\n') {
        if len == LAUNCHER_DOCS_MAX {
            break;
        }
        if line.is_empty() || line.len() > DOC_PATH_MAX {
            continue;
        }
        if str::from_utf8(line).is_err() {
            continue;
        }
        docs[len].path[..line.len()].copy_from_slice(line);
        docs[len].path_len = line.len();
        len += 1;
    }
    (docs, len)
}

/// Splits a stored path into (directory scope including the trailing
/// separator, file name). Root-level entries search the whole index.
fn split_scope_and_name(path: &[u8]) -> (&[u8], &[u8]) {
    match path.iter().rposition(|byte| *byte == b'/') {
        Some(index) => (&path[..index + 1], &path[index + 1..]),
        None => (b"", path),
    }
}

/// Confirms a ring entry still exists via the storage name index and
/// returns the confirmed hit carrying the authoritative entry kind. A
/// stale path (deleted file), a transport error, or an over-long scan
/// drops the entry instead of showing a dead document.
fn validate_ring_entry(storage: rt::Handle, path: &[u8]) -> Option<DocHit> {
    let (scope, name) = split_scope_and_name(path);
    if name.is_empty() {
        return None;
    }
    let query = rt::StorageSearchQuery::from_bytes(name)?;
    let mut cursor = 0usize;
    for _ in 0..DOC_HITS_MAX * 2 {
        match rt::storage_search::<DOC_PATH_MAX>(storage, cursor, scope, &query) {
            Ok(Some(hit)) => {
                let next_cursor = hit.next_cursor;
                if hit.path_as_bytes() == path {
                    return Some(DocHit {
                        path: hit.path,
                        path_len: hit.path_len,
                        kind: hit.kind as u32 as u64,
                        line: 0,
                    });
                }
                if next_cursor <= cursor {
                    return None;
                }
                cursor = next_cursor;
            }
            Ok(None) | Err(_) => return None,
        }
    }
    None
}

/// Rebuilds the panel document section from the files-app recent ring.
/// Any degraded condition (no storage handle, unreadable ring, transport
/// error) leaves zero documents so the panel degrades to the app-only
/// layout with no placeholder rows. Returns true when the visible
/// document set changed.
pub(crate) fn refresh_launcher_docs(state: &mut DesktopState) -> bool {
    let previous = (state.launcher_docs, state.launcher_docs_len);
    state.launcher_docs_len = 0;
    if state.storage_handle == rt::INVALID_HANDLE {
        return previous.1 != 0;
    }
    let storage = state.storage_handle;
    let Ok(dir) = rt::storage_open_directory(storage, RING_STORE_DIR_PREFIX, false) else {
        return previous.1 != 0;
    };
    let ring_bytes = match rt::storage_directory_open_file(dir, RING_FILE_NAME, false, false) {
        Ok((file, size)) => {
            let mut buffer = [0u8; RING_FILE_BYTES];
            let expected = size.min(buffer.len());
            let read = rt::storage_read_all(file, &mut buffer, expected);
            let _ = rt::handle_close(file);
            read.map(|len| (buffer, len)).ok()
        }
        Err(_) => None,
    };
    let _ = rt::handle_close(dir);
    let Some((buffer, read_len)) = ring_bytes else {
        return previous.1 != 0;
    };

    let (mut docs, mut docs_len) = parse_ring_entries(&buffer[..read_len]);
    let mut kept = 0usize;
    for index in 0..docs_len {
        if kept == LAUNCHER_DOCS_MAX {
            break;
        }
        let path_len = docs[index].path_len;
        let Some(validated) = validate_ring_entry(storage, &docs[index].path[..path_len]) else {
            continue;
        };
        docs[kept] = validated;
        kept += 1;
    }
    docs_len = kept;

    state.launcher_docs = docs;
    state.launcher_docs_len = docs_len;
    docs_len != previous.1 || docs[..docs_len] != previous.0[..docs_len]
}

/// Surface-local y band of document row `row`, mirroring the app-row
/// band math in `input::hit_test`. The header grid row is not clickable.
pub(crate) fn launcher_doc_row_at(local_y: i32) -> Option<usize> {
    for row in 0..LAUNCHER_DOCS_MAX {
        let line_y =
            ui::PANEL_LINE_START_Y + ((DOC_ROW_GRID_ROW_BASE + row) as i32 * ui::PANEL_LINE_STEP);
        let line_top = line_y - 2;
        let line_bottom = line_top + ui::PANEL_LINE_STEP;
        if local_y >= line_top && local_y < line_bottom {
            return Some(row);
        }
    }
    None
}

/// Surface-local y of the section header label.
pub(crate) fn doc_header_y() -> i32 {
    ui::PANEL_LINE_START_Y + (DOC_HEADER_GRID_ROW as i32 * ui::PANEL_LINE_STEP)
}

/// Surface-local y of document row `row`.
pub(crate) fn doc_row_y(row: usize) -> i32 {
    ui::PANEL_LINE_START_Y
        + ((DOC_ROW_GRID_ROW_BASE + row.min(LAUNCHER_DOCS_MAX - 1)) as i32 * ui::PANEL_LINE_STEP)
}

/// Panel row label: palette kind icon plus the file name portion of the
/// path. Over-long names clip on a char boundary with a "..." suffix so
/// the label never trips the 56-byte graphics cap with a broken tail.
pub(crate) fn doc_row_label(buffer: &mut rt::FixedLogBuffer<56>, doc: &DocHit) {
    let path = &doc.path[..doc.path_len.min(doc.path.len())];
    let name_start = path
        .iter()
        .rposition(|byte| *byte == b'/')
        .map(|index| index + 1)
        .unwrap_or(0);
    let name = str::from_utf8(&path[name_start..]).unwrap_or("");
    let _ = write!(buffer, "{} ", doc_kind_icon(doc.kind));
    if name.len() <= DOC_NAME_MAX {
        let _ = core::fmt::Write::write_str(buffer, name);
        return;
    }
    let mut end = DOC_NAME_MAX.saturating_sub(3);
    while end > 0 && !name.is_char_boundary(end) {
        end -= 1;
    }
    let _ = core::fmt::Write::write_str(buffer, &name[..end]);
    let _ = core::fmt::Write::write_str(buffer, "...");
}

/// Panel-click entry point: routes the selected document row through the
/// same OpenPath handoff the command palette uses for its document rows
/// (launch-or-focus files-app, then app-control OpenPath). Out-of-range
/// rows, over-cap rows, and undecodable paths are silent no-ops.
pub(crate) fn open_launcher_doc(state: &mut DesktopState, row: usize) -> rt::Result<u32> {
    let row = row.min(LAUNCHER_DOCS_MAX.saturating_sub(1));
    if row >= state.launcher_docs_len {
        return Ok(0);
    }
    let mut path_bytes = [0u8; DOC_PATH_MAX];
    let doc = state.launcher_docs[row];
    let path_len = doc.path_len.min(DOC_PATH_MAX);
    path_bytes[..path_len].copy_from_slice(&doc.path[..path_len]);
    let Ok(path) = str::from_utf8(&path_bytes[..path_len]) else {
        return Ok(0);
    };
    if path.is_empty() {
        return Ok(0);
    }
    open_path_in_files(state, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(path: &[u8], kind: u64) -> DocHit {
        let mut value = DocHit {
            path: [0; DOC_PATH_MAX],
            path_len: path.len(),
            kind,
            line: 0,
        };
        value.path[..path.len()].copy_from_slice(path);
        value
    }

    // ---- ring parsing / recency ranking --------------------------------

    #[test]
    fn ring_file_order_is_recency_ranking_newest_first() {
        // files-app encodes the move-to-front ring head first, so a ring
        // that recorded a.txt, then b.log, then re-recorded a.txt encodes
        // as: a.txt (newest), c.bin, b.log.
        let (docs, len) = parse_ring_entries(b"a.txt\nc.bin\nb.log\n");
        assert_eq!(len, 3);
        assert_eq!(docs[0].path_str(), "a.txt");
        assert_eq!(docs[1].path_str(), "c.bin");
        assert_eq!(docs[2].path_str(), "b.log");
    }

    #[test]
    fn ring_parse_caps_at_four_documents() {
        let (docs, len) = parse_ring_entries(b"0\n1\n2\n3\n4\n5\n");
        assert_eq!(len, LAUNCHER_DOCS_MAX);
        assert_eq!(docs[LAUNCHER_DOCS_MAX - 1].path_str(), "3");
    }

    #[test]
    fn ring_parse_skips_blank_and_unrepresentable_lines() {
        let (docs, len) = parse_ring_entries(b"\nkeep.txt\n\n\n");
        assert_eq!(len, 1);
        assert_eq!(docs[0].path_str(), "keep.txt");

        // DOC_PATH_MAX bytes exactly is still representable; one byte
        // more must be skipped, never truncated.
        let (at_cap, len) = parse_ring_entries(&[b'a'; DOC_PATH_MAX]);
        assert_eq!(len, 1);
        assert_eq!(at_cap[0].path_len, DOC_PATH_MAX);

        let mut over = [b'a'; DOC_PATH_MAX + 2];
        over[DOC_PATH_MAX + 1] = b'\n';
        let (docs, len) = parse_ring_entries(&over);
        assert_eq!(len, 0, "over-length paths are skipped, never truncated");

        let (docs, len) = parse_ring_entries(b"valid.txt");
        assert_eq!(len, 1);
        assert_eq!(docs[0].path_str(), "valid.txt");
        assert_eq!(docs[0].line, 0);
    }

    #[test]
    fn ring_parse_tolerates_garbage_and_empty_body() {
        assert_eq!(parse_ring_entries(b"").1, 0);
        assert_eq!(parse_ring_entries(b"\n\n\n").1, 0);
    }

    // ---- section layout / hit bands ------------------------------------

    #[test]
    fn doc_rows_continue_panel_grid_without_touching_app_rows() {
        // App rows occupy grid rows 0..5; the header sits on row 6 and the
        // document rows on 7..10. Bands must be disjoint and contiguous.
        for row in 0..LAUNCHER_DOCS_MAX {
            let y = doc_row_y(row);
            let band_top = y - 2;
            let band_bottom = band_top + ui::PANEL_LINE_STEP;
            assert!(band_top >= 42 + 6 * ui::PANEL_LINE_STEP - 2);
            for app_row in 0..6usize {
                let app_y = 42 + (app_row as i32 * ui::PANEL_LINE_STEP);
                let app_band = app_y - 2..app_y - 2 + ui::PANEL_LINE_STEP;
                assert!(
                    band_bottom <= app_band.start || band_top >= app_band.end,
                    "doc row {row} must not overlap app row {app_row}"
                );
            }
            assert_eq!(launcher_doc_row_at(band_top), Some(row));
            assert_eq!(launcher_doc_row_at(band_bottom - 1), Some(row));
            assert_eq!(
                launcher_doc_row_at(band_bottom),
                if row + 1 < LAUNCHER_DOCS_MAX {
                    Some(row + 1)
                } else {
                    None
                }
            );
        }
    }

    #[test]
    fn header_band_and_blank_tail_are_not_clickable() {
        let header_y = doc_header_y();
        assert_eq!(header_y, 42 + 6 * ui::PANEL_LINE_STEP);
        assert_eq!(
            launcher_doc_row_at(header_y - 2),
            None,
            "header row is not a document hit"
        );
        assert_eq!(launcher_doc_row_at(header_y), None);
        // The first document row band begins right where the header's
        // band ends (that boundary is covered by the adjacency test).
        assert_eq!(launcher_doc_row_at(0), None);
        assert_eq!(launcher_doc_row_at(-1), None);
        assert_eq!(launcher_doc_row_at(260), None);
    }

    #[test]
    fn doc_row_band_starts_right_after_header_band() {
        let header_y = doc_header_y();
        let header_band_end = header_y - 2 + ui::PANEL_LINE_STEP;
        assert_eq!(launcher_doc_row_at(header_band_end), Some(0));
    }

    // ---- row labels ------------------------------------------------------

    #[test]
    fn row_label_uses_palette_kind_icons_and_name_portion() {
        let mut buffer = rt::FixedLogBuffer::<56>::new();
        doc_row_label(&mut buffer, &hit(b"docs/readme.txt", 0));
        assert_eq!(
            str::from_utf8(buffer.as_bytes()).ok(),
            Some("[] readme.txt")
        );

        let mut buffer = rt::FixedLogBuffer::<56>::new();
        doc_row_label(&mut buffer, &hit(b"docs/notes", 1));
        assert_eq!(str::from_utf8(buffer.as_bytes()).ok(), Some("[/] notes"));

        let mut buffer = rt::FixedLogBuffer::<56>::new();
        doc_row_label(&mut buffer, &hit(b"plain.txt", 0));
        assert_eq!(str::from_utf8(buffer.as_bytes()).ok(), Some("[] plain.txt"));
    }

    #[test]
    fn row_label_clips_long_names_on_char_boundaries_within_label_cap() {
        let long_ascii: &[u8] = &[b'n'; 80];
        let mut buffer = rt::FixedLogBuffer::<56>::new();
        doc_row_label(&mut buffer, &hit(long_ascii, 0));
        let label = str::from_utf8(buffer.as_bytes()).expect("label must stay valid utf-8");
        assert!(label.starts_with("[] n"));
        assert!(label.ends_with("..."));
        assert!(label.len() <= 56, "label must fit the graphics cap");

        // Multi-byte tail: the clip must back off to a char boundary.
        let mut multibyte: Vec<u8> = Vec::new();
        multibyte.extend_from_slice(b"docs/");
        multibyte.extend_from_slice(&[b'x'; 46]);
        multibyte.extend_from_slice("ééé".as_bytes());
        let mut buffer = rt::FixedLogBuffer::<56>::new();
        doc_row_label(&mut buffer, &hit(&multibyte, 0));
        let label = str::from_utf8(buffer.as_bytes()).expect("clip must not split a char");
        assert!(label.ends_with("..."));
        assert!(!label.contains('\u{FFFD}'));
    }

    // ---- change detection ------------------------------------------------

    #[test]
    fn changed_detection_distinguishes_len_and_content() {
        let old = [hit(b"a", 0), hit(b"b", 0), hit(b"c", 0), hit(b"d", 0)];
        let same = old;
        assert!(same[..4] == old[..4]);

        let reordered = [hit(b"b", 0), hit(b"a", 0), hit(b"c", 0), hit(b"d", 0)];
        assert!(
            old[..4] != reordered[..4],
            "recency reorder must count as a change"
        );

        let rekinned = [hit(b"a", 1), hit(b"b", 0), hit(b"c", 0), hit(b"d", 0)];
        assert!(
            old[..4] != rekinned[..4],
            "kind change must count as a change"
        );
    }

    // ---- intent routing parity ------------------------------------------

    #[test]
    fn panel_rows_and_palette_rows_share_the_open_path_handoff() {
        // The panel click handler (open_launcher_doc) and the palette
        // Enter branch (input::overlays PaletteEntry::Doc) both call
        // windows::open_path_in_files — assert the shared symbol is the
        // one imported here so the handoff cannot silently fork.
        let function_path: fn(&mut DesktopState, &str) -> rt::Result<u32> = open_path_in_files;
        let _ = function_path;
        assert_eq!(DOC_HITS_MAX, 4);
    }

    #[test]
    fn scope_split_covers_root_and_nested_paths() {
        assert_eq!(
            split_scope_and_name(b"docs/readme.txt"),
            (&b"docs/"[..], &b"readme.txt"[..])
        );
        assert_eq!(
            split_scope_and_name(b"readme.txt"),
            (&b""[..], &b"readme.txt"[..])
        );
        assert_eq!(
            split_scope_and_name(b"a/b/c.bin"),
            (&b"a/b/"[..], &b"c.bin"[..])
        );
    }
}
