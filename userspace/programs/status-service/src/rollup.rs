use serviceos_abi::{ManagerServicePhase, RawMessage, StatusHealth};

pub(crate) const ROLLUP_LIST_CAP: usize = 2;
pub(crate) const ROLLUP_OFFENDERS: usize = 2;
pub(crate) const ROLLUP_REPLY_WORDS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RollupEntry {
    pub service_id: u32,
    pub health: StatusHealth,
    pub phase: ManagerServicePhase,
    pub restarts: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RollupSummary {
    pub total: usize,
    pub counts: [usize; 6],
    pub restarting_count: usize,
    pub degraded_ids: [u32; ROLLUP_LIST_CAP],
    pub degraded_len: usize,
    pub restarting_ids: [u32; ROLLUP_LIST_CAP],
    pub restarting_len: usize,
    pub offenders: [(u32, u64); ROLLUP_OFFENDERS],
    /// Memory-pressure level discriminant (0 normal / 1 tight / 2 critical)
    /// and lifetime transition count, mirrored from the kernel pressure feed.
    pub pressure_level: u32,
    pub pressure_transitions: u64,
}

impl RollupSummary {
    pub(crate) const fn empty() -> Self {
        Self {
            total: 0,
            counts: [0; 6],
            restarting_count: 0,
            degraded_ids: [0; ROLLUP_LIST_CAP],
            degraded_len: 0,
            restarting_ids: [0; ROLLUP_LIST_CAP],
            restarting_len: 0,
            offenders: [(0, 0); ROLLUP_OFFENDERS],
            pressure_level: 0,
            pressure_transitions: 0,
        }
    }
}

fn health_slot(health: StatusHealth) -> usize {
    match health {
        StatusHealth::Unknown => 0,
        StatusHealth::Healthy => 1,
        StatusHealth::Degraded => 2,
        StatusHealth::Failing => 3,
        StatusHealth::Recovering => 4,
        StatusHealth::Dormant => 5,
    }
}

pub(crate) fn is_restarting_phase(phase: ManagerServicePhase) -> bool {
    matches!(phase, ManagerServicePhase::Backoff)
}

pub(crate) fn compute_rollup(entries: &[RollupEntry]) -> RollupSummary {
    let mut summary = RollupSummary::empty();
    summary.total = entries.len();

    let mut offenders = [(0u32, 0u64); ROLLUP_OFFENDERS];
    for entry in entries {
        summary.counts[health_slot(entry.health)] += 1;
        if is_restarting_phase(entry.phase) {
            summary.restarting_count += 1;
            if summary.restarting_len < ROLLUP_LIST_CAP {
                summary.restarting_ids[summary.restarting_len] = entry.service_id;
                summary.restarting_len += 1;
            }
        }
        if entry.health == StatusHealth::Degraded && summary.degraded_len < ROLLUP_LIST_CAP {
            summary.degraded_ids[summary.degraded_len] = entry.service_id;
            summary.degraded_len += 1;
        }
        if entry.restarts > 0 {
            insert_offender(&mut offenders, (entry.service_id, entry.restarts));
        }
    }

    summary.offenders = offenders;
    summary
}

/// Keeps the top `ROLLUP_OFFENDERS` by restarts descending, ties broken by
/// ascending service id. Slots past the last real offender stay `(0, 0)`.
fn insert_offender(offenders: &mut [(u32, u64); ROLLUP_OFFENDERS], candidate: (u32, u64)) {
    for index in 0..ROLLUP_OFFENDERS {
        if better(candidate, offenders[index]) {
            shift_down(offenders, index);
            offenders[index] = candidate;
            return;
        }
    }
}

fn better(candidate: (u32, u64), incumbent: (u32, u64)) -> bool {
    if incumbent.0 == 0 && incumbent.1 == 0 {
        return true;
    }
    candidate.1 > incumbent.1 || (candidate.1 == incumbent.1 && candidate.0 < incumbent.0)
}

fn shift_down(offenders: &mut [(u32, u64); ROLLUP_OFFENDERS], from: usize) {
    let mut index = ROLLUP_OFFENDERS - 1;
    while index > from {
        offenders[index] = offenders[index - 1];
        index -= 1;
    }
}

fn pack_pair(first: u32, second: u32) -> u64 {
    first as u64 | ((second as u64) << 32)
}

pub(crate) fn fill_snapshot_reply(
    reply: &mut RawMessage,
    heartbeat_count: u64,
    last_tick: u64,
    summary: &RollupSummary,
) {
    reply.words[0] = heartbeat_count;
    reply.words[1] = last_tick;
    reply.words[2] = summary.total as u64;
    for slot in 0..summary.counts.len() {
        reply.words[3 + slot] = summary.counts[slot] as u64;
    }
    reply.words[9] = summary.restarting_count as u64;
    reply.words[10] = summary.degraded_len as u64;
    reply.words[11] = pack_pair(summary.degraded_ids[0], summary.degraded_ids[1]);
    reply.words[12] = summary.restarting_len as u64;
    reply.words[13] = pack_pair(summary.restarting_ids[0], summary.restarting_ids[1]);
    reply.words[14] = pack_pair(
        summary.offenders[0].0,
        summary.offenders[0].1.min(u32::MAX as u64) as u32,
    );
    reply.words[15] = pack_pair(
        summary.offenders[1].0,
        summary.offenders[1].1.min(u32::MAX as u64) as u32,
    );
    reply.word_count = ROLLUP_REPLY_WORDS as u32;
}

#[cfg(test)]
mod tests {
    use super::*;

    const SID_A: u32 = 3;
    const SID_B: u32 = 5;
    const SID_C: u32 = 7;

    fn entry(
        service_id: u32,
        health: StatusHealth,
        phase: ManagerServicePhase,
        restarts: u64,
    ) -> RollupEntry {
        RollupEntry {
            service_id,
            health,
            phase,
            restarts,
        }
    }

    #[test]
    fn empty_snapshot_yields_zero_rollup() {
        let summary = compute_rollup(&[]);
        assert_eq!(summary.total, 0);
        assert_eq!(summary.counts, [0; 6]);
        assert_eq!(summary.restarting_count, 0);
        assert_eq!(summary.degraded_len, 0);
        assert_eq!(summary.restarting_len, 0);
        assert_eq!(summary.offenders, [(0, 0); ROLLUP_OFFENDERS]);

        let mut reply = RawMessage::empty(0);
        fill_snapshot_reply(&mut reply, 4, 99, &summary);
        assert_eq!(reply.word_count as usize, ROLLUP_REPLY_WORDS);
        assert_eq!(reply.words[0], 4);
        assert_eq!(reply.words[1], 99);
        assert_eq!(reply.words[2], 0);
        for word in reply.words[3..16].iter() {
            assert_eq!(*word, 0);
        }
    }

    #[test]
    fn all_healthy_counts_and_totals() {
        let entries = [
            entry(SID_A, StatusHealth::Healthy, ManagerServicePhase::Ready, 0),
            entry(SID_B, StatusHealth::Healthy, ManagerServicePhase::Ready, 0),
            entry(
                SID_C,
                StatusHealth::Healthy,
                ManagerServicePhase::Starting,
                0,
            ),
        ];
        let summary = compute_rollup(&entries);
        assert_eq!(summary.total, 3);
        assert_eq!(summary.counts[health_slot(StatusHealth::Healthy)], 3);
        for slot in [0usize, 2, 3, 4, 5] {
            assert_eq!(summary.counts[slot], 0);
        }
        assert_eq!(summary.restarting_count, 0);
        assert_eq!(summary.offenders, [(0, 0); ROLLUP_OFFENDERS]);
    }

    #[test]
    fn mixed_states_collect_lists_and_counts() {
        let entries = [
            entry(SID_C, StatusHealth::Healthy, ManagerServicePhase::Ready, 0),
            entry(
                SID_A,
                StatusHealth::Degraded,
                ManagerServicePhase::Degraded,
                0,
            ),
            entry(
                SID_B,
                StatusHealth::Recovering,
                ManagerServicePhase::Backoff,
                3,
            ),
            entry(11, StatusHealth::Failing, ManagerServicePhase::Exited, 0),
            entry(13, StatusHealth::Dormant, ManagerServicePhase::Dormant, 0),
            entry(
                17,
                StatusHealth::Unknown,
                ManagerServicePhase::WaitingDependencies,
                0,
            ),
        ];
        let summary = compute_rollup(&entries);
        assert_eq!(summary.total, 6);
        assert_eq!(summary.counts[health_slot(StatusHealth::Degraded)], 1);
        assert_eq!(summary.counts[health_slot(StatusHealth::Recovering)], 1);
        assert_eq!(summary.counts[health_slot(StatusHealth::Failing)], 1);
        assert_eq!(summary.counts[health_slot(StatusHealth::Dormant)], 1);
        assert_eq!(summary.counts[health_slot(StatusHealth::Unknown)], 1);
        assert_eq!(summary.counts[health_slot(StatusHealth::Healthy)], 1);
        assert_eq!(summary.degraded_len, 1);
        assert_eq!(summary.degraded_ids[0], SID_A);
        assert_eq!(summary.restarting_count, 1);
        assert_eq!(summary.restarting_len, 1);
        assert_eq!(summary.restarting_ids[0], SID_B);

        let mut reply = RawMessage::empty(0);
        fill_snapshot_reply(&mut reply, 0, 0, &summary);
        assert_eq!(reply.words[9], 1);
        assert_eq!(reply.words[11] as u32, SID_A);
        assert_eq!(reply.words[13] as u32, SID_B);
        assert_eq!(reply.words[14] >> 32, 3);
        assert_eq!(reply.words[14] as u32, SID_B);
        assert_eq!(reply.words[15], 0);
    }

    #[test]
    fn worst_offenders_rank_and_break_ties_by_id() {
        let entries = [
            entry(SID_C, StatusHealth::Healthy, ManagerServicePhase::Ready, 7),
            entry(SID_A, StatusHealth::Healthy, ManagerServicePhase::Ready, 7),
            entry(SID_B, StatusHealth::Healthy, ManagerServicePhase::Ready, 2),
            entry(19, StatusHealth::Healthy, ManagerServicePhase::Ready, 0),
        ];
        let summary = compute_rollup(&entries);
        assert_eq!(summary.offenders, [(SID_A, 7), (SID_C, 7)]);
    }

    #[test]
    fn worst_offenders_ignore_zero_and_fill_in_order() {
        let entries = [
            entry(21, StatusHealth::Healthy, ManagerServicePhase::Ready, 0),
            entry(23, StatusHealth::Healthy, ManagerServicePhase::Ready, 1),
        ];
        let summary = compute_rollup(&entries);
        assert_eq!(summary.offenders, [(23, 1), (0, 0)]);
    }

    #[test]
    fn capped_lists_keep_first_two_seen() {
        let entries = [
            entry(31, StatusHealth::Degraded, ManagerServicePhase::Backoff, 0),
            entry(33, StatusHealth::Degraded, ManagerServicePhase::Backoff, 0),
            entry(35, StatusHealth::Degraded, ManagerServicePhase::Backoff, 0),
        ];
        let summary = compute_rollup(&entries);
        assert_eq!(summary.degraded_len, ROLLUP_LIST_CAP);
        assert_eq!(summary.degraded_ids, [31, 33]);
        assert_eq!(summary.restarting_len, ROLLUP_LIST_CAP);
        assert_eq!(summary.restarting_ids, [31, 33]);
        assert_eq!(summary.restarting_count, 3);
    }
}
