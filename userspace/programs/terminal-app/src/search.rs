//! Interactive Ctrl-R reverse history search for the terminal front-end.
//!
//! The app mirrors each pane's editable line and submitted commands from the
//! key/text events it forwards, so it can run the shell-service search state
//! machine locally: typed characters refine the query, repeated Ctrl-R cycles
//! older matches, Esc cancels untouched, Enter rewrites the service-side line
//! with the accepted match and executes it.

use super::*;
use serviceos_shell_service::history_search::HistorySource;

/// Begin a search on the focused pane (or cycle its current one older).
/// Returns whether screen state changed.
pub(crate) fn begin_or_cycle(state: &mut TerminalState) -> bool {
    let Some(tab) = crate::tabs::active_tab_ref(state) else {
        return false;
    };
    let pane_index = tab.tree.focused.min(tab.pane_count.saturating_sub(1));
    if !matches!(state.search.as_ref(), Some(overlay) if overlay.pane_index == pane_index) {
        state.search = Some(SearchOverlay {
            pane_index,
            inner: fresh_search(),
        });
        return true;
    }
    let mut overlay = match state.search.take() {
        Some(overlay) => overlay,
        None => return false,
    };
    let changed = match focused_pane_for_search(state, pane_index) {
        Some(pane) => {
            overlay.inner.cycle_older(&pane.history);
            true
        }
        None => false,
    };
    state.search = Some(overlay);
    changed
}

type SearchOverlayInner = serviceos_shell_service::history_search::HistorySearch;

fn fresh_search() -> SearchOverlayInner {
    let mut inner = SearchOverlayInner::new();
    inner.begin();
    inner
}

fn focused_pane_for_search(
    state: &mut TerminalState,
    pane_index: usize,
) -> Option<&mut TerminalPane> {
    let tab = state.tabs.get_mut(state.active_tab)?;
    if !tab.occupied || pane_index >= tab.pane_count {
        return None;
    }
    tab.panes.get_mut(pane_index)
}

/// Leave search mode without touching session or mirror state.
pub(crate) fn cancel(state: &mut TerminalState) -> bool {
    state.search.take().is_some()
}

/// Feed one text scalar while a search is active on the focused pane.
/// Returns Ok(true) when the event was consumed by the search.
pub(crate) fn handle_text(state: &mut TerminalState, ch: char) -> rt::Result<bool> {
    let mut overlay = match state.search.take() {
        Some(overlay) => overlay,
        None => return Ok(false),
    };
    enum Outcome {
        Refine,
        Accepted,
        Released,
    }
    let outcome = (|| -> rt::Result<Outcome> {
        let pane_index = overlay.pane_index;
        let Some(tab) = state.tabs.get_mut(state.active_tab) else {
            return Ok(Outcome::Released);
        };
        if !tab.occupied || pane_index >= tab.pane_count {
            return Ok(Outcome::Released);
        }
        let pane = &mut tab.panes[pane_index];
        if ch == '\n' || ch == '\r' {
            accept_match(pane, &mut overlay.inner)?;
            return Ok(Outcome::Accepted);
        }
        let byte = ch as u32;
        if byte <= 0x7e {
            overlay.inner.refine(byte as u8, &pane.history);
        }
        Ok(Outcome::Refine) // non-printables are swallowed while searching
    })();
    match outcome? {
        Outcome::Refine => {
            state.search = Some(overlay);
            Ok(true)
        }
        Outcome::Accepted => Ok(true),
        Outcome::Released => Ok(false),
    }
}

/// Backspace while searching edits the query, not the line.
pub(crate) fn handle_backspace(state: &mut TerminalState) -> bool {
    if state.search.is_none() {
        return false;
    }
    let mut overlay = match state.search.take() {
        Some(overlay) => overlay,
        None => return false,
    };
    let mut handled = false;
    if let Some(pane) = focused_pane_for_search(state, overlay.pane_index) {
        overlay.inner.pop_query(&pane.history);
        handled = true;
    }
    state.search = Some(overlay);
    handled
}

/// Rewrite the service-side line with the accepted match and execute it:
/// enough backspaces to erase the stashed pre-search input, then the match
/// text, then newline.
fn accept_match(pane: &mut TerminalPane, search: &mut SearchOverlayInner) -> rt::Result<()> {
    let mut matched = [0u8; serviceos_shell_service::history_search::MAX_ENTRY_BYTES];
    let Some(match_len) = search.matched_entry(&pane.history, &mut matched) else {
        return Ok(()); // nothing ever matched; behave like cancel
    };
    let stash_len = pane.input_mirror_len.min(MIRROR_LINE_BYTES);
    let mut payload = [0u8; MIRROR_LINE_BYTES * 2 + 1];
    let mut len = 0usize;
    for _ in 0..stash_len {
        payload[len] = 0x7f;
        len += 1;
    }
    payload[len..len + match_len].copy_from_slice(&matched[..match_len]);
    len += match_len;
    payload[len] = b'\n';
    len += 1;
    rt::terminal_session_send_input(pane.session_handle, &payload[..len])?;
    // Mirror now shows the accepted command, which also enters this pane's
    // history ring exactly like a normally submitted line would.
    pane.mirror_reset(&payload[stash_len..stash_len + match_len]);
    Ok(())
}

// ---------------------------------------------------------------------------
// Input mirror maintenance (normal typing path)
// ---------------------------------------------------------------------------

/// A printable character was forwarded to the session: extend the mirror.
pub(crate) fn note_char(pane: &mut TerminalPane, ch: char) {
    let mut bytes = [0u8; 4];
    let encoded = ch.encode_utf8(&mut bytes).as_bytes();
    for byte in encoded {
        if *byte >= 0x20 && *byte <= 0x7e && pane.input_mirror_len < MIRROR_LINE_BYTES {
            pane.input_mirror[pane.input_mirror_len] = *byte;
            pane.input_mirror_len += 1;
        }
    }
}

/// The submitted line was executed: fold the mirror into history.
pub(crate) fn commit_line(pane: &mut TerminalPane) {
    let mut line = [0u8; MIRROR_LINE_BYTES];
    let trimmed = pane.trimmed_mirror();
    let len = trimmed.len().min(line.len());
    line[..len].copy_from_slice(&trimmed[..len]);
    pane.history.push(&line[..len]);
    pane.input_mirror_len = 0;
    pane.hist_view = None;
    pane.hist_stash_len = 0;
}

/// A backspace was forwarded to the session: shorten the mirror.
pub(crate) fn note_backspace(pane: &mut TerminalPane) {
    if pane.input_mirror_len > 0 {
        pane.input_mirror_len -= 1;
        pane.input_mirror[pane.input_mirror_len] = 0;
    }
}

/// Replicate the service-side arrow-up recall against the mirrored ring so
/// subsequent commits record exactly what will execute.
pub(crate) fn history_up(pane: &mut TerminalPane) {
    if pane.history.count() == 0 {
        return;
    }
    let next_view = match pane.hist_view {
        None => {
            pane.hist_stash[..pane.input_mirror_len]
                .copy_from_slice(&pane.input_mirror[..pane.input_mirror_len]);
            pane.hist_stash_len = pane.input_mirror_len;
            pane.history.count() - 1
        }
        Some(0) => 0,
        Some(view) => view - 1,
    };
    load_history_view(pane, next_view);
}

/// Arrow-down: step toward the newest recall, restoring the stash at the end.
pub(crate) fn history_down(pane: &mut TerminalPane) {
    let Some(current) = pane.hist_view else {
        return;
    };
    if current + 1 >= pane.history.count() {
        pane.hist_view = None;
        pane.input_mirror[..pane.hist_stash_len]
            .copy_from_slice(&pane.hist_stash[..pane.hist_stash_len]);
        pane.input_mirror_len = pane.hist_stash_len;
        pane.hist_stash_len = 0;
        return;
    }
    load_history_view(pane, current + 1);
}

fn load_history_view(pane: &mut TerminalPane, view: usize) {
    let mut entry = [0u8; PANE_HISTORY_LINE_BYTES];
    let Some(len) = pane.history.entry(view, &mut entry) else {
        return;
    };
    pane.input_mirror[..len].copy_from_slice(&entry[..len]);
    pane.input_mirror_len = len;
    pane.hist_view = Some(view);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane() -> TerminalPane {
        TerminalPane::empty()
    }

    #[test]
    fn mirror_tracks_typing_backspace_and_commit() {
        let mut pane = pane();
        for ch in "echo hi".chars() {
            note_char(&mut pane, ch);
        }
        assert_eq!(pane.trimmed_mirror(), b"echo hi");
        note_backspace(&mut pane);
        assert_eq!(pane.trimmed_mirror(), b"echo h");
        note_char(&mut pane, 'i');
        commit_line(&mut pane);
        assert_eq!(pane.input_mirror_len, 0);
        assert_eq!(pane.history.count(), 1);
        let mut buffer = [0u8; PANE_HISTORY_LINE_BYTES];
        assert_eq!(pane.history.entry(0, &mut buffer), Some(7));
        assert_eq!(&buffer[..7], b"echo hi");
    }

    #[test]
    fn commits_trim_whitespace_and_collapse_duplicates() {
        let mut pane = pane();
        for ch in "  ls \t".chars() {
            note_char(&mut pane, ch);
        }
        commit_line(&mut pane);
        for ch in "ls".chars() {
            note_char(&mut pane, ch);
        }
        commit_line(&mut pane);
        assert_eq!(pane.history.count(), 1, "consecutive duplicates collapse");
    }

    #[test]
    fn arrow_recall_round_trips_through_stash() {
        let mut pane = pane();
        for ch in "first".chars() {
            note_char(&mut pane, ch);
        }
        commit_line(&mut pane);
        for ch in "second".chars() {
            note_char(&mut pane, ch);
        }
        commit_line(&mut pane);

        for ch in "partial".chars() {
            note_char(&mut pane, ch);
        }
        history_up(&mut pane);
        assert_eq!(pane.trimmed_mirror(), b"second");
        history_up(&mut pane);
        assert_eq!(pane.trimmed_mirror(), b"first");
        history_down(&mut pane);
        assert_eq!(pane.trimmed_mirror(), b"second");
        history_down(&mut pane);
        assert_eq!(
            pane.trimmed_mirror(),
            b"partial",
            "stash restored past the newest recall"
        );
        commit_line(&mut pane);
        assert_eq!(pane.history.count(), 3);
    }

    #[test]
    fn search_finds_and_accepts_history_matches() {
        let mut pane = pane();
        for line in ["boot", "cargo build", "cargo test"] {
            for ch in line.chars() {
                note_char(&mut pane, ch);
            }
            commit_line(&mut pane);
        }

        let mut search = SearchOverlayInner::new();
        search.begin();
        for byte in b"cargo" {
            assert!(search.refine(*byte, &pane.history));
        }
        assert_eq!(search.match_order(), Some(2));
        search.cycle_older(&pane.history);
        assert_eq!(search.match_order(), Some(1));

        let mut buffer = [0u8; 32];
        let len = search.matched_entry(&pane.history, &mut buffer).unwrap();
        assert_eq!(&buffer[..len], b"cargo build");

        // Accept path: backspace stash then replay the matched line.
        for ch in "junk".chars() {
            note_char(&mut pane, ch);
        }
        assert_eq!(pane.input_mirror_len, 4);
        let stash_len = pane.input_mirror_len;
        let mut payload = [0u8; MIRROR_LINE_BYTES * 2 + 1];
        for slot in payload.iter_mut().take(stash_len) {
            *slot = 0x7f;
        }
        payload[stash_len..stash_len + len].copy_from_slice(&buffer[..len]);
        payload[stash_len + len] = b'\n';
        let total = stash_len + len + 1;
        pane.mirror_reset(&payload[stash_len..stash_len + len]);
        assert_eq!(pane.trimmed_mirror(), b"cargo build");
        assert_eq!(total, stash_len + len + 1);
    }
}
