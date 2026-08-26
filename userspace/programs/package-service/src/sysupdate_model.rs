//! Pure whole-system update ("sysupdate") transaction model shared between
//! the `no_std` service binary and host unit tests. A system update is an
//! ordered set of per-package updates recorded as ONE operation-journal
//! entry (`JOURNAL_SYSUPDATE`) plus a persisted transaction file, so an
//! interrupted apply participates in the existing stale-journal recovery.

/// Maximum packages in one system-update transaction (matches the package
/// slot count).
pub const MAX_SYSUPDATE_STEPS: usize = 12;

/// Transaction state machine / commit-marker states.
pub const TXN_STATE_PLANNING: u32 = 0;
pub const TXN_STATE_APPLYING: u32 = 1;
pub const TXN_STATE_COMMITTING: u32 = 2;
pub const TXN_STATE_COMMITTED: u32 = 3;
pub const TXN_STATE_ROLLING_BACK: u32 = 4;
pub const TXN_STATE_ROLLED_BACK: u32 = 5;
pub const TXN_STATE_FAILED: u32 = 6;

pub const TXN_FLAG_ROLLED_BACK: u64 = 1;

/// Bounded system-update history ring (newest last).
pub const SYSUPDATE_HISTORY_CAP: usize = 8;
/// History rows carried in a single IPC reply.
pub const SYSUPDATE_HISTORY_REPLY_ROWS: usize = 5;

pub fn txn_state_name(state: u32) -> &'static str {
    match state {
        TXN_STATE_PLANNING => "planning",
        TXN_STATE_APPLYING => "applying",
        TXN_STATE_COMMITTING => "committing",
        TXN_STATE_COMMITTED => "committed",
        TXN_STATE_ROLLING_BACK => "rolling-back",
        TXN_STATE_ROLLED_BACK => "rolled-back",
        TXN_STATE_FAILED => "failed",
        _ => "unknown",
    }
}

/// Allowed commit-marker transitions:
/// planning -> applying -> committing -> committed -> rolling-back ->
/// rolled-back, with applying -> failed and failed -> applying (resume
/// retry) as the only side branches.
pub fn txn_transition_allowed(from: u32, to: u32) -> bool {
    matches!(
        (from, to),
        (
            TXN_STATE_PLANNING,
            TXN_STATE_APPLYING | TXN_STATE_FAILED
        ) | (TXN_STATE_APPLYING, TXN_STATE_COMMITTING | TXN_STATE_FAILED)
            | (TXN_STATE_COMMITTING, TXN_STATE_COMMITTED | TXN_STATE_FAILED)
            | (TXN_STATE_COMMITTED, TXN_STATE_ROLLING_BACK)
            | (TXN_STATE_ROLLING_BACK, TXN_STATE_ROLLED_BACK | TXN_STATE_FAILED)
            // Resume retry of a failed or interrupted run.
            | (TXN_STATE_FAILED, TXN_STATE_APPLYING | TXN_STATE_ROLLING_BACK)
    )
}

/// Terminal markers: the transaction reached a durable outcome.
pub fn txn_is_final(state: u32) -> bool {
    matches!(state, TXN_STATE_COMMITTED | TXN_STATE_ROLLED_BACK)
}

/// True when the recorded step cursor means "nothing left to do".
pub fn txn_steps_remaining(done: usize, total: usize) -> usize {
    done.min(total).abs_diff(total)
}

/// Copy `ids` into `ordered` ascending by id and deduplicating repeats.
/// Returns the ordered count. Ordering is deterministic so plan, apply,
/// rollback, and crash-resume all walk the same sequence.
pub fn order_ids(ids: &[u32], ordered: &mut [u32; MAX_SYSUPDATE_STEPS]) -> usize {
    let mut count = 0usize;
    for value in ids.iter().copied() {
        if count >= ordered.len() {
            break;
        }
        ordered[count] = value;
        count += 1;
    }
    if count > 1 {
        // Insertion sort: tiny fixed bound, no alloc.
        for index in 1..count {
            let key = ordered[index];
            let mut slot = index;
            while slot > 0 && ordered[slot - 1] > key {
                ordered[slot] = ordered[slot - 1];
                slot -= 1;
            }
            ordered[slot] = key;
        }
    }
    // Dedupe adjacent repeats, keeping the first occurrence.
    let mut kept = 0usize;
    for index in 0..count {
        if index == 0 || ordered[index] != ordered[kept - 1] {
            ordered[kept] = ordered[index];
            kept += 1;
        }
    }
    kept
}

/// Reverse-rollback plan: the restore order is exactly the applied order
/// walked backwards, so the most recently updated package is restored first.
pub fn reverse_ids(
    ids: &[u32; MAX_SYSUPDATE_STEPS],
    count: usize,
    reversed: &mut [u32; MAX_SYSUPDATE_STEPS],
) -> usize {
    let count = count.min(ids.len()).min(reversed.len());
    for index in 0..count {
        reversed[index] = ids[count - 1 - index];
    }
    count
}

/// Serialize the transaction file:
/// `version=1`, `state=<state>|<done>|<total>`, then one `id=<n>` line per
/// planned package in execution order.
pub fn encode_txn_file(
    state: u32,
    done: usize,
    ids: &[u32; MAX_SYSUPDATE_STEPS],
    count: usize,
    buffer: &mut ModelTextBuffer<512>,
) {
    let _ = write!(buffer, "version=1\n");
    let _ = write!(buffer, "state={}|{}|{}\n", state, done, count);
    for index in 0..count.min(ids.len()) {
        let _ = write!(buffer, "id={}\n", ids[index]);
    }
}

pub struct ParsedTxn {
    pub state: u32,
    /// Completed steps. For APPLYING this counts applied packages from the
    /// start; for ROLLING_BACK it counts completed reverse steps from the
    /// end, so resume always continues where the file says it stopped.
    pub done: usize,
    pub total: usize,
    pub ids: [u32; MAX_SYSUPDATE_STEPS],
    pub count: usize,
}

impl ParsedTxn {
    pub const fn empty() -> Self {
        Self {
            state: TXN_STATE_PLANNING,
            done: 0,
            total: 0,
            ids: [0; MAX_SYSUPDATE_STEPS],
            count: 0,
        }
    }
}

/// Parse a transaction file produced by [`encode_txn_file`]. Returns `None`
/// on malformed input (bad version/header/ids) so callers treat it as
/// absent rather than guessing.
pub fn parse_txn_file(text: &str) -> Option<ParsedTxn> {
    let mut parsed = ParsedTxn::empty();
    let mut saw_version = false;
    let mut saw_state = false;
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let Some(value) = line.strip_prefix("version=") {
            if value != "1" {
                return None;
            }
            saw_version = true;
        } else if let Some(payload) = line.strip_prefix("state=") {
            let mut parts = payload.split('|');
            parsed.state = parts.next()?.parse::<u32>().ok()?;
            parsed.done = parts.next()?.parse::<usize>().ok()?;
            parsed.total = parts.next()?.parse::<usize>().ok()?;
            saw_state = true;
        } else if let Some(payload) = line.strip_prefix("id=") {
            let id = payload.parse::<u32>().ok()?;
            if parsed.count >= MAX_SYSUPDATE_STEPS {
                return None;
            }
            parsed.ids[parsed.count] = id;
            parsed.count += 1;
        }
    }
    if !saw_version || !saw_state || parsed.count != parsed.total {
        return None;
    }
    Some(parsed)
}

/// Serialize one history row: `hist=<seq>|<tick>|<applied>|<rolled_back>`.
pub fn encode_history_line(
    seq: u64,
    tick: u64,
    applied: u64,
    rolled_back: bool,
    buffer: &mut ModelTextBuffer<128>,
) {
    let _ = write!(
        buffer,
        "hist={}|{}|{}|{}\n",
        seq,
        tick,
        applied,
        u32::from(rolled_back),
    );
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HistoryRow {
    pub seq: u64,
    pub tick: u64,
    pub applied: u64,
    pub rolled_back: bool,
}

/// Parse history rows oldest-first and keep only the newest
/// [`SYSUPDATE_HISTORY_CAP`] entries (ring trim).
pub fn parse_history_rows(text: &str) -> ([HistoryRow; SYSUPDATE_HISTORY_CAP], usize) {
    let mut all = (
        [HistoryRow {
            seq: 0,
            tick: 0,
            applied: 0,
            rolled_back: false,
        }; SYSUPDATE_HISTORY_CAP],
        0usize,
    );
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let Some(payload) = line.strip_prefix("hist=") else {
            continue;
        };
        let mut parts = payload.split('|');
        let (Some(seq), Some(tick), Some(applied), Some(flag)) = (
            parts.next().and_then(|v| v.parse::<u64>().ok()),
            parts.next().and_then(|v| v.parse::<u64>().ok()),
            parts.next().and_then(|v| v.parse::<u64>().ok()),
            parts.next().and_then(|v| v.parse::<u32>().ok()),
        ) else {
            continue;
        };
        let row = HistoryRow {
            seq,
            tick,
            applied,
            rolled_back: flag != 0,
        };
        push_history_row(&mut all, row);
    }
    all
}

/// Append one row, evicting the oldest when the ring is full.
pub fn push_history_row(
    ring: &mut ([HistoryRow; SYSUPDATE_HISTORY_CAP], usize),
    row: HistoryRow,
) {
    let (rows, count) = ring;
    if *count < SYSUPDATE_HISTORY_CAP {
        rows[*count] = row;
        *count += 1;
    } else {
        rows.copy_within(1..SYSUPDATE_HISTORY_CAP, 0);
        rows[SYSUPDATE_HISTORY_CAP - 1] = row;
    }
}

use core::fmt::{self, Write};

/// Minimal fixed-capacity text sink so the pure model stays independent of
/// the userspace runtime crate while remaining `no_std`.
pub struct ModelTextBuffer<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> ModelTextBuffer<N> {
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }
}

impl<const N: usize> Write for ModelTextBuffer<N> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let rest = &mut self.bytes[self.len.min(N)..];
        let copy = rest.len().min(text.len());
        rest[..copy].copy_from_slice(&text.as_bytes()[..copy]);
        self.len = self.len.min(N) + copy;
        if copy < text.len() {
            return Err(fmt::Error);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordering_sorts_and_dedupes() {
        let mut out = [0u32; MAX_SYSUPDATE_STEPS];
        assert_eq!(order_ids(&[40, 7, 40, 12, 7], &mut out), 3);
        assert_eq!(&out[..3], &[7, 12, 40]);
    }

    #[test]
    fn ordering_clamps_to_capacity_and_handles_empty() {
        let mut out = [0u32; MAX_SYSUPDATE_STEPS];
        assert_eq!(order_ids(&[], &mut out), 0);
        let many = [9u32, 8, 7, 6, 5, 4, 3, 2, 1, 0, 11, 10, 99];
        assert_eq!(order_ids(&many, &mut out), MAX_SYSUPDATE_STEPS);
        assert_eq!(out[MAX_SYSUPDATE_STEPS - 1], 11);
    }

    #[test]
    fn reverse_plan_walks_apply_order_backwards() {
        let mut ids = [0u32; MAX_SYSUPDATE_STEPS];
        let count = order_ids(&[30, 10, 20], &mut ids);
        let mut back = [0u32; MAX_SYSUPDATE_STEPS];
        assert_eq!(reverse_ids(&ids, count, &mut back), 3);
        assert_eq!(&back[..3], &[30, 20, 10]);
    }

    #[test]
    fn reverse_plan_empty_and_over_count_are_safe() {
        let ids = [5u32; MAX_SYSUPDATE_STEPS];
        let mut back = [0u32; MAX_SYSUPDATE_STEPS];
        assert_eq!(reverse_ids(&ids, 0, &mut back), 0);
        assert_eq!(reverse_ids(&ids, 999, &mut back), MAX_SYSUPDATE_STEPS);
    }

    #[test]
    fn state_machine_allows_only_documented_transitions() {
        assert!(txn_transition_allowed(TXN_STATE_PLANNING, TXN_STATE_APPLYING));
        assert!(txn_transition_allowed(TXN_STATE_APPLYING, TXN_STATE_COMMITTING));
        assert!(txn_transition_allowed(TXN_STATE_COMMITTING, TXN_STATE_COMMITTED));
        assert!(txn_transition_allowed(TXN_STATE_COMMITTED, TXN_STATE_ROLLING_BACK));
        assert!(txn_transition_allowed(TXN_STATE_ROLLING_BACK, TXN_STATE_ROLLED_BACK));
        assert!(txn_transition_allowed(TXN_STATE_APPLYING, TXN_STATE_FAILED));
        assert!(txn_transition_allowed(TXN_STATE_FAILED, TXN_STATE_APPLYING));
        assert!(txn_transition_allowed(TXN_STATE_FAILED, TXN_STATE_ROLLING_BACK));
        assert!(!txn_transition_allowed(TXN_STATE_PLANNING, TXN_STATE_COMMITTED));
        assert!(!txn_transition_allowed(TXN_STATE_COMMITTED, TXN_STATE_APPLYING));
        assert!(!txn_transition_allowed(TXN_STATE_ROLLED_BACK, TXN_STATE_APPLYING));
        assert!(!txn_transition_allowed(TXN_STATE_COMMITTED, TXN_STATE_COMMITTED));
        assert!(!txn_transition_allowed(TXN_STATE_APPLYING, TXN_STATE_ROLLED_BACK));
    }

    #[test]
    fn final_states_are_exactly_committed_and_rolled_back() {
        assert!(txn_is_final(TXN_STATE_COMMITTED));
        assert!(txn_is_final(TXN_STATE_ROLLED_BACK));
        assert!(!txn_is_final(TXN_STATE_APPLYING));
        assert!(!txn_is_final(TXN_STATE_FAILED));
    }

    #[test]
    fn steps_remaining_counts_forward_only() {
        assert_eq!(txn_steps_remaining(0, 4), 4);
        assert_eq!(txn_steps_remaining(3, 4), 1);
        assert_eq!(txn_steps_remaining(4, 4), 0);
        // A cursor past the total never goes negative.
        assert_eq!(txn_steps_remaining(9, 4), 0);
    }

    #[test]
    fn txn_file_round_trips() {
        let mut ids = [0u32; MAX_SYSUPDATE_STEPS];
        let count = order_ids(&[42, 7], &mut ids);
        let mut buffer = ModelTextBuffer::<512>::new();
        encode_txn_file(TXN_STATE_APPLYING, 1, &ids, count, &mut buffer);
        let text = core::str::from_utf8(buffer.as_bytes()).unwrap();
        let parsed = parse_txn_file(text).expect("roundtrip");
        assert_eq!(parsed.state, TXN_STATE_APPLYING);
        assert_eq!(parsed.done, 1);
        assert_eq!(parsed.count, 2);
        assert_eq!(&parsed.ids[..2], &[7, 42]);
    }

    #[test]
    fn txn_file_rejects_garbage() {
        assert!(parse_txn_file("").is_none());
        assert!(parse_txn_file("version=2\nstate=1|0|0\n").is_none());
        assert!(parse_txn_file("state=1|0|1\nid=5\n").is_none());
        assert!(parse_txn_file("version=1\nstate=1|0|2\nid=5\n").is_none());
        assert!(parse_txn_file("version=1\nstate=x|0|0\n").is_none());
        assert!(parse_txn_file("version=1\nstate=1|0|1\nid=nope\n").is_none());
    }

    #[test]
    fn txn_file_caps_entries() {
        let mut ids = [0u32; MAX_SYSUPDATE_STEPS];
        for slot in 0..MAX_SYSUPDATE_STEPS {
            ids[slot] = slot as u32;
        }
        // A header promising more entries than the file carries is invalid;
        // so is a payload longer than the fixed capacity.
        let mut over = ModelTextBuffer::<512>::new();
        let _ = write!(over, "version=1\nstate=1|0|{}\n", MAX_SYSUPDATE_STEPS + 1);
        for slot in 0..MAX_SYSUPDATE_STEPS {
            let _ = write!(over, "id={}\n", ids[slot]);
        }
        assert!(parse_txn_file(core::str::from_utf8(over.as_bytes()).unwrap()).is_none());
    }

    #[test]
    fn history_ring_keeps_newest_cap_rows() {
        let mut ring = ([HistoryRow { seq: 0, tick: 0, applied: 0, rolled_back: false }; SYSUPDATE_HISTORY_CAP], 0usize);
        for seq in 0..(SYSUPDATE_HISTORY_CAP as u64 + 3) {
            push_history_row(
                &mut ring,
                HistoryRow {
                    seq,
                    tick: seq * 10,
                    applied: 2,
                    rolled_back: seq % 2 == 1,
                },
            );
        }
        let (rows, count) = ring;
        assert_eq!(count, SYSUPDATE_HISTORY_CAP);
        assert_eq!(rows[0].seq, 3);
        // Newest row (seq 3+8-1 = 10) is even, hence a clean commit; the
        // second-newest (seq 9) is an odd, rolled-back transaction.
        assert_eq!(rows[SYSUPDATE_HISTORY_CAP - 1].seq, 10);
        assert!(!rows[SYSUPDATE_HISTORY_CAP - 1].rolled_back);
        assert!(rows[SYSUPDATE_HISTORY_CAP - 2].seq == 9);
        assert!(rows[SYSUPDATE_HISTORY_CAP - 2].rolled_back);
    }

    #[test]
    fn history_codec_round_trips_with_trim() {
        let mut body = ModelTextBuffer::<1024>::new();
        for seq in 0..6u64 {
            let mut line = ModelTextBuffer::<128>::new();
            encode_history_line(seq, seq * 100, 3, seq == 5, &mut line);
            let _ = body.write_str(core::str::from_utf8(line.as_bytes()).unwrap());
        }
        let text = core::str::from_utf8(body.as_bytes()).unwrap();
        let (rows, count) = parse_history_rows(text);
        assert_eq!(count, 6);
        assert_eq!(rows[5].tick, 500);
        assert!(rows[5].rolled_back);
        assert_eq!(rows[0].applied, 3);
    }

    #[test]
    fn history_parser_skips_malformed_lines() {
        let (rows, count) = parse_history_rows("junk\nhist=1|20|2|0\nhist=broken\nhist=2|30|1|1\n");
        assert_eq!(count, 2);
        assert_eq!(rows[0].seq, 1);
        assert_eq!(rows[1].seq, 2);
        assert!(rows[1].rolled_back);
    }

    #[test]
    fn state_names_cover_codes() {
        assert_eq!(txn_state_name(TXN_STATE_COMMITTED), "committed");
        assert_eq!(txn_state_name(TXN_STATE_ROLLING_BACK), "rolling-back");
        assert_eq!(txn_state_name(99), "unknown");
    }
}
