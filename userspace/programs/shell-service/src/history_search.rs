//! Incremental reverse history search (Ctrl-R style) state machine.
//!
//! Pure logic over a bounded history snapshot: the front-end feeds printable
//! query bytes, backspaces, and "cycle older" events; the machine tracks the
//! current match position against a [`HistorySource`] so it stays decoupled
//! from any particular storage layout (shell rings, pane mirrors, ...).
//! Host tests cover the full transition surface.

/// Query capacity; longer refinements are refused rather than truncated so
/// the displayed query always equals the searched query.
pub const MAX_QUERY_BYTES: usize = 32;
/// Entry scratch size; matches the shell's `HISTORY_LINE_BYTES`.
pub const MAX_ENTRY_BYTES: usize = 128;

/// Read-only newest-last history view. Order indexes are oldest-first
/// (`count - 1` is the newest), mirroring the operator-session ring.
pub trait HistorySource {
    /// Number of retained entries.
    fn count(&self) -> usize;
    /// Copy entry `order` into `out`, returning its length (None out of
    /// range), exactly like the session ring's accessor.
    fn entry(&self, order: usize, out: &mut [u8]) -> Option<usize>;
}

impl<T: HistorySource> HistorySource for &T {
    fn count(&self) -> usize {
        (**self).count()
    }

    fn entry(&self, order: usize, out: &mut [u8]) -> Option<usize> {
        (**self).entry(order, out)
    }
}

/// Incremental reverse-search state. Inactive until [`HistorySearch::begin`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HistorySearch {
    active: bool,
    failed: bool,
    query_len: usize,
    query: [u8; MAX_QUERY_BYTES],
    match_order: Option<usize>,
}

impl HistorySearch {
    pub const fn new() -> Self {
        Self {
            active: false,
            failed: false,
            query_len: 0,
            query: [0; MAX_QUERY_BYTES],
            match_order: None,
        }
    }

    /// Enter search mode: empty query, no match yet, nothing failing.
    pub fn begin(&mut self) {
        self.active = true;
        self.failed = false;
        self.query_len = 0;
        self.query = [0; MAX_QUERY_BYTES];
        self.match_order = None;
    }

    /// Leave search mode without side effects; the caller keeps its own
    /// pre-search input line.
    pub fn cancel(&mut self) {
        self.active = false;
        self.failed = false;
        self.match_order = None;
    }

    /// Accept the current match and leave search mode. Returns the accepted
    /// order index (None when nothing ever matched).
    pub fn accept(&mut self) -> Option<usize> {
        let accepted = self.match_order;
        self.active = false;
        self.failed = false;
        self.query_len = 0;
        self.query = [0; MAX_QUERY_BYTES];
        self.match_order = None;
        accepted
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn is_failed(&self) -> bool {
        self.failed
    }

    pub fn query(&self) -> &[u8] {
        &self.query[..self.query_len]
    }

    pub fn match_order(&self) -> Option<usize> {
        self.match_order
    }

    /// Feed one typed byte. Printable ASCII refines the query incrementally:
    /// the current match is kept when it still contains the refined query,
    /// otherwise the search continues from that position downward only. A
    /// refinement with no match sets the failed flag but keeps the last good
    /// match displayed. Non-printable bytes are ignored. Returns whether the
    /// byte refined.
    pub fn refine<S: HistorySource>(&mut self, byte: u8, history: &S) -> bool {
        if !self.active
            || history.count() == 0
            || !(0x20..=0x7e).contains(&byte)
            || self.query_len >= MAX_QUERY_BYTES
        {
            return false;
        }
        self.query[self.query_len] = byte;
        self.query_len += 1;
        let query = self.query();
        // Extending the query can only keep or move the match older: re-check
        // the previous match first, then scan strictly older entries.
        if let Some(order) = find_from(history, query, self.match_order) {
            self.match_order = Some(order);
            self.failed = false;
        } else {
            // Failed refinement: keep the previous match visible and flag the
            // miss, mirroring readline's "failing" prompt.
            if self.match_order.is_none() {
                self.match_order = newest_containing(history, query);
            }
            self.failed = true;
        }
        true
    }

    /// Remove the last query byte (backspace while searching) and re-resolve
    /// the match newest-first; clears the failed flag.
    pub fn pop_query<S: HistorySource>(&mut self, history: &S) -> bool {
        if !self.active || self.query_len == 0 {
            return false;
        }
        self.query_len -= 1;
        self.query[self.query_len] = 0;
        self.failed = false;
        self.match_order =
            find_from(history, self.query(), self.match_order.map(|_| history.count()));
        true
    }

    /// Repeat Ctrl-R: step to the next older match for the current query,
    /// wrapping past the oldest hit back to the newest so repeated presses
    /// cycle. An empty query simply pins the newest entry.
    pub fn cycle_older<S: HistorySource>(&mut self, history: &S) {
        if !self.active || history.count() == 0 {
            return;
        }
        if self.query().is_empty() {
            self.match_order = Some(history.count() - 1);
            self.failed = false;
            return;
        }
        let start = match self.match_order {
            // Already at the oldest entry (or unpositioned): a fresh
            // newest-first scan implements the wrap-around step.
            Some(0) | None => None,
            Some(order) => Some(order - 1),
        };
        if let Some(order) = find_from(history, self.query(), start) {
            self.match_order = Some(order);
            self.failed = false;
        }
    }

    /// Copy the currently matched entry into `out`; returns its length.
    pub fn matched_entry<S: HistorySource>(&self, history: &S, out: &mut [u8]) -> Option<usize> {
        history.entry(self.match_order?, out)
    }
}

fn newest_containing<S: HistorySource>(history: &S, query: &[u8]) -> Option<usize> {
    let mut scratch = [0u8; MAX_ENTRY_BYTES];
    let count = history.count();
    for order in (0..count).rev() {
        if let Some(len) = history.entry(order, &mut scratch) {
            if contains(&scratch[..len], query) {
                return Some(order);
            }
        }
    }
    None
}

/// Reverse scan oldest-index-descending from `start` (inclusive; None means
/// newest) for the first entry containing `query`. An empty query matches the
/// starting entry itself when one exists.
fn find_from<S: HistorySource>(history: &S, query: &[u8], start: Option<usize>) -> Option<usize> {
    let count = history.count();
    if count == 0 {
        return None;
    }
    let mut scratch = [0u8; MAX_ENTRY_BYTES];
    let mut order = start.unwrap_or(count - 1).min(count - 1);
    loop {
        if let Some(len) = history.entry(order, &mut scratch) {
            if contains(&scratch[..len], query) {
                return Some(order);
            }
        }
        if order == 0 {
            return None;
        }
        order -= 1;
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return needle.is_empty();
    }
    for offset in 0..=(haystack.len() - needle.len()) {
        if &haystack[offset..offset + needle.len()] == needle {
            return true;
        }
    }
    false
}

/// Render the search status line: ``(reverse-i-search)`query': match`` with a
/// ``failing `` prefix while the last refinement missed. Returns bytes written.
pub fn render_search_line(search: &HistorySearch, matched: &[u8], out: &mut [u8]) -> usize {
    let prefix: &str = if search.is_failed() { "failing " } else { "" };
    let mut written = 0usize;
    let mut push = |bytes: &[u8]| {
        for byte in bytes {
            if written < out.len() {
                out[written] = *byte;
                written += 1;
            }
        }
    };
    push(b"(");
    push(prefix.as_bytes());
    push(b"reverse-i-search)`");
    push(search.query());
    push(b"': ");
    push(matched);
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixed newest-last history fixture.
    struct Fixture {
        entries: [&'static str; 4],
        count: usize,
    }

    impl Fixture {
        fn new(entries: &[&'static str]) -> Self {
            let mut fixture = Self {
                entries: ["", "", "", ""],
                count: entries.len(),
            };
            fixture.entries[..entries.len()].copy_from_slice(entries);
            fixture
        }
    }

    impl HistorySource for Fixture {
        fn count(&self) -> usize {
            self.count
        }

        fn entry(&self, order: usize, out: &mut [u8]) -> Option<usize> {
            let text = self.entries.get(order)?;
            let bytes = text.as_bytes();
            let len = bytes.len().min(out.len());
            out[..len].copy_from_slice(&bytes[..len]);
            Some(len)
        }
    }

    #[test]
    fn empty_query_matches_newest_entry() {
        let history = Fixture::new(&["one", "two", "three"]);
        assert_eq!(find_from(&history, b"", None), Some(2));
    }

    #[test]
    fn reverse_scan_prefers_newest_containing_match() {
        let history = Fixture::new(&["ls -l", "echo hi", "ls"]);
        assert_eq!(find_from(&history, b"ls", None), Some(2));
        assert_eq!(find_from(&history, b"echo", None), Some(1));
        assert_eq!(find_from(&history, b"missing", None), None);
    }

    #[test]
    fn refine_walks_matches_incrementally() {
        let history = Fixture::new(&["cargo build", "cargo test", "boot", "cargo fmt"]);
        let mut search = HistorySearch::new();
        search.begin();
        assert!(search.is_active());
        assert_eq!(search.match_order(), None);

        assert!(search.refine(b'c', &history));
        assert_eq!(search.match_order(), Some(3));
        for byte in b"argo" {
            assert!(search.refine(*byte, &history));
            assert_eq!(search.match_order(), Some(3));
        }

        // "cargo t" skips order 3 ("cargo fmt") and lands on "cargo test".
        for byte in b" t" {
            assert!(search.refine(*byte, &history));
        }
        assert_eq!(search.match_order(), Some(1));
        assert!(!search.is_failed());
        assert_eq!(search.query(), b"cargo t");
    }

    #[test]
    fn failed_refinement_flags_and_keeps_last_match() {
        let history = Fixture::new(&["alpha", "beta"]);
        let mut search = HistorySearch::new();
        search.begin();
        for byte in b"beta" {
            search.refine(*byte, &history);
        }
        assert_eq!(search.match_order(), Some(1));
        assert!(search.refine(b'?', &history));
        assert!(search.is_failed());
        assert_eq!(search.match_order(), Some(1), "last good match stays shown");
        assert_eq!(search.query(), b"beta?");
    }

    #[test]
    fn non_printable_and_overflow_bytes_are_refused() {
        let history = Fixture::new(&["cmd"]);
        let mut search = HistorySearch::new();
        search.begin();
        assert!(!search.refine(0x03, &history));
        assert!(!search.refine(0x7f, &history));
        for _ in 0..MAX_QUERY_BYTES {
            assert!(search.refine(b'x', &history));
        }
        assert!(!search.refine(b'y', &history), "query capacity held");
        assert_eq!(search.query().len(), MAX_QUERY_BYTES);
    }

    #[test]
    fn pop_query_restores_previous_match_state() {
        let history = Fixture::new(&["alpha", "alphabet", "alpha beta"]);
        let mut search = HistorySearch::new();
        search.begin();
        for byte in b"alphabet" {
            assert!(search.refine(*byte, &history));
        }
        // "alphabet" only fits order 1; order 2 is "alpha beta".
        assert_eq!(search.match_order(), Some(1));
        // Drop back to "alphabe": still unique to order 1.
        assert!(search.pop_query(&history));
        assert_eq!(search.match_order(), Some(1));
        // Drop to "alpha": both other entries fit, newest wins, failure cleared.
        for _ in 0..2 {
            assert!(search.pop_query(&history));
        }
        assert!(!search.is_failed());
        assert_eq!(search.match_order(), Some(2));
        assert_eq!(search.query(), b"alpha");
    }

    #[test]
    fn cycle_older_steps_then_wraps_to_newest() {
        let history = Fixture::new(&["ls", "ls -l", "ls -la", "echo"]);
        let mut search = HistorySearch::new();
        search.begin();
        for byte in b"ls" {
            search.refine(*byte, &history);
        }
        assert_eq!(search.match_order(), Some(2));
        search.cycle_older(&history);
        assert_eq!(search.match_order(), Some(1));
        search.cycle_older(&history);
        assert_eq!(search.match_order(), Some(0));
        search.cycle_older(&history);
        assert_eq!(
            search.match_order(),
            Some(2),
            "cycling past the oldest wraps to the newest"
        );
    }

    #[test]
    fn cycle_older_with_empty_query_selects_newest() {
        let history = Fixture::new(&["a", "b"]);
        let mut search = HistorySearch::new();
        search.begin();
        search.cycle_older(&history);
        assert_eq!(search.match_order(), Some(1));
    }

    #[test]
    fn accept_returns_match_and_deactivates() {
        let history = Fixture::new(&["one", "two"]);
        let mut search = HistorySearch::new();
        search.begin();
        search.refine(b't', &history);
        assert_eq!(search.accept(), Some(1));
        assert!(!search.is_active());
        assert_eq!(search.match_order(), None);
        assert!(search.query().is_empty());

        let mut fresh = HistorySearch::new();
        fresh.begin();
        assert_eq!(fresh.accept(), None, "nothing matched yet");
    }

    #[test]
    fn cancel_clears_state_without_touching_history() {
        let history = Fixture::new(&["one"]);
        let mut search = HistorySearch::new();
        search.begin();
        search.refine(b'o', &history);
        search.cancel();
        assert!(!search.is_active());
        assert_eq!(search.match_order(), None);
        assert!(!search.is_failed());
    }

    #[test]
    fn operations_on_inactive_search_are_inert() {
        let history = Fixture::new(&["x"]);
        let mut search = HistorySearch::new();
        assert!(!search.refine(b'x', &history));
        assert!(!search.pop_query(&history));
        search.cycle_older(&history);
        assert_eq!(search.match_order(), None);
    }

    #[test]
    fn empty_history_is_safe_everywhere() {
        let history = Fixture::new(&[]);
        let mut search = HistorySearch::new();
        search.begin();
        assert!(!search.refine(b'a', &history), "no history: nothing refines");
        search.cycle_older(&history);
        assert_eq!(find_from(&history, b"a", None), None);
        assert_eq!(search.accept(), None);
    }

    #[test]
    fn substring_matching_is_case_sensitive_byte_exact() {
        let history = Fixture::new(&["LIST files", "list files"]);
        assert_eq!(find_from(&history, b"LIST", None), Some(0));
        assert_eq!(find_from(&history, b"list", None), Some(1));
        assert_eq!(find_from(&history, b"files", None), Some(1));
    }

    #[test]
    fn matched_entry_copies_current_hit() {
        let history = Fixture::new(&["first", "second hit"]);
        let mut search = HistorySearch::new();
        search.begin();
        search.refine(b'd', &history);
        let mut buffer = [0u8; MAX_ENTRY_BYTES];
        let len = search.matched_entry(&history, &mut buffer).unwrap();
        assert_eq!(&buffer[..len], b"second hit");
    }

    #[test]
    fn render_search_line_formats_query_and_failure_prefix() {
        let history = Fixture::new(&["systemctl status", "stats"]);
        let mut search = HistorySearch::new();
        search.begin();
        for byte in b"stat" {
            search.refine(*byte, &history);
        }
        let mut buffer = [0u8; 64];
        let len = render_search_line(&search, b"stats", &mut buffer);
        assert_eq!(&buffer[..len], b"(reverse-i-search)`stat': stats");

        assert!(search.refine(b'z', &history));
        let len = render_search_line(&search, b"stats", &mut buffer);
        assert_eq!(
            &buffer[..len],
            b"(failing reverse-i-search)`statz': stats"
        );
    }
}
