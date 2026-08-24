//! Pure installer/updater model shared between the `no_std` service binary
//! and host unit tests: progress phase math, operation-journal staleness
//! classification, and the `[<version>][@<source>]` argument grammar used by
//! install/update source selection.

pub const PROGRESS_PHASE_COUNT: usize = 5;

pub const PROGRESS_PHASE_RESOLVE: u8 = 0;
pub const PROGRESS_PHASE_MATERIALIZE: u8 = 1;
pub const PROGRESS_PHASE_VERIFY: u8 = 2;
pub const PROGRESS_PHASE_ACTIVATE: u8 = 3;
pub const PROGRESS_PHASE_PERSIST: u8 = 4;

pub const JOURNAL_NONE: u32 = 0;
pub const JOURNAL_INSTALL: u32 = 1;
pub const JOURNAL_UPDATE: u32 = 2;
pub const JOURNAL_REMOVE: u32 = 3;
pub const JOURNAL_ROLLBACK: u32 = 4;

/// Maintenance action word extending `PackageMaintenanceAction` for the
/// interrupted-update recovery flow (service + shell agree on this value).
pub const MAINTENANCE_ACTION_RECOVER: u64 = 4;

/// Operation reply trigger/outcome codes.
pub const TRIGGER_OPERATOR: u64 = 1;
pub const TRIGGER_AUTO_RESTORE: u64 = 2;
pub const RECOVERY_OUTCOME_NONE: u64 = 0;
pub const RECOVERY_OUTCOME_RESUMED: u64 = 1;
pub const RECOVERY_OUTCOME_DISCARDED: u64 = 2;
pub const RECOVERY_OUTCOME_RESUME_FAILED: u64 = 3;

pub fn phase_name(phase: u8) -> &'static str {
    match phase {
        PROGRESS_PHASE_RESOLVE => "resolve",
        PROGRESS_PHASE_MATERIALIZE => "materialize",
        PROGRESS_PHASE_VERIFY => "verify",
        PROGRESS_PHASE_ACTIVATE => "activate",
        PROGRESS_PHASE_PERSIST => "persist",
        _ => "unknown",
    }
}

pub fn journal_action_name(action: u32) -> &'static str {
    match action {
        JOURNAL_INSTALL => "install",
        JOURNAL_UPDATE => "update",
        JOURNAL_REMOVE => "remove",
        JOURNAL_ROLLBACK => "rollback",
        _ => "none",
    }
}

/// A journal entry recorded before an interruptible step is stale exactly
/// when it still names a pending action; the service persists a cleared
/// journal after every completed operation, so any pending entry observed at
/// startup means the previous run was interrupted mid-operation.
pub fn journal_is_stale(pending_action: u32) -> bool {
    pending_action != JOURNAL_NONE
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProgressTracker {
    pub phase: u8,
    pub step: u32,
    pub total_steps: u32,
}

impl ProgressTracker {
    pub const fn new(total_steps: u32) -> Self {
        Self {
            phase: PROGRESS_PHASE_RESOLVE,
            step: 0,
            total_steps,
        }
    }

    pub fn enter_phase(&mut self, phase: u8) {
        if (phase as usize) < PROGRESS_PHASE_COUNT {
            self.phase = phase;
        }
    }

    pub fn complete_step(&mut self) {
        self.step = self.step.saturating_add(1);
    }

    /// Whole-operation percent: each of the five phases owns an equal share
    /// and the current phase's share is filled proportionally by the number
    /// of completed steps relative to the total step budget.
    pub fn percent(&self) -> u32 {
        progress_percent(self.phase, self.step, self.total_steps)
    }

    pub fn pack(&self) -> u64 {
        pack_progress(self.phase, self.step, self.total_steps)
    }
}

pub fn progress_percent(phase: u8, step: u32, total_steps: u32) -> u32 {
    if (phase as usize) >= PROGRESS_PHASE_COUNT || total_steps == 0 {
        return 0;
    }
    let per_phase = 100 / PROGRESS_PHASE_COUNT as u32;
    let step_share = (step.min(total_steps) * per_phase) / total_steps;
    phase as u32 * per_phase + step_share
}

/// Pack a progress snapshot into one reply word:
/// bits [7:0] phase, bits [23:8] step, bits [39:24] total steps.
pub fn pack_progress(phase: u8, step: u32, total_steps: u32) -> u64 {
    (phase as u64 & 0xff) | ((step as u64 & 0xffff) << 8) | ((total_steps as u64 & 0xffff) << 24)
}

pub fn unpack_progress(word: u64) -> (u8, u32, u32) {
    (
        (word & 0xff) as u8,
        ((word >> 8) & 0xffff) as u32,
        ((word >> 24) & 0xffff) as u32,
    )
}

/// Split an install/update version argument into the explicit version part
/// and the optional `@source` repository name. The source suffix is parsed
/// from the last `@`; both parts are trimmed of surrounding whitespace and
/// an empty source or empty remainder is treated as absent.
pub fn split_version_source(argument: &str) -> (&str, Option<&str>) {
    let Some((version, source)) = argument.rsplit_once('@') else {
        return (argument.trim(), None);
    };
    let source = source.trim();
    let version = version.trim();
    if source.is_empty() {
        return (version, None);
    }
    (version, Some(source))
}

/// True when a candidate version slot (identified by its repository index)
/// satisfies the requested source restriction. `resolved_source` is the
/// repository index matched by name, `None` when no source was requested.
pub fn source_permits(
    resolved_source: Option<usize>,
    candidate_repo_index: usize,
    candidate_occupied: bool,
) -> bool {
    match resolved_source {
        None => true,
        Some(source_index) => candidate_occupied && candidate_repo_index == source_index,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_covers_all_phases_evenly() {
        assert_eq!(progress_percent(PROGRESS_PHASE_RESOLVE, 0, 5), 0);
        assert_eq!(progress_percent(PROGRESS_PHASE_RESOLVE, 5, 5), 20);
        assert_eq!(progress_percent(PROGRESS_PHASE_MATERIALIZE, 0, 5), 20);
        assert_eq!(progress_percent(PROGRESS_PHASE_VERIFY, 3, 5), 52);
        assert_eq!(progress_percent(PROGRESS_PHASE_PERSIST, 5, 5), 100);
    }

    #[test]
    fn percent_handles_zero_total_and_unknown_phase() {
        assert_eq!(progress_percent(PROGRESS_PHASE_RESOLVE, 1, 0), 0);
        assert_eq!(progress_percent(9, 1, 5), 0);
    }

    #[test]
    fn tracker_walks_phases_and_clamps() {
        let mut tracker = ProgressTracker::new(3);
        assert_eq!(tracker.percent(), 0);
        tracker.complete_step();
        tracker.complete_step();
        tracker.enter_phase(PROGRESS_PHASE_ACTIVATE);
        // activate share (60) plus the 2-of-3 steps carried into it (13).
        assert_eq!(tracker.percent(), 73);
        tracker.enter_phase(200);
        assert_eq!(tracker.phase, PROGRESS_PHASE_ACTIVATE);
        tracker.complete_step();
        tracker.complete_step();
        assert_eq!(tracker.percent(), 80);
    }

    #[test]
    fn progress_word_round_trips() {
        let word = pack_progress(PROGRESS_PHASE_VERIFY, 7, 12);
        assert_eq!(unpack_progress(word), (PROGRESS_PHASE_VERIFY, 7, 12));
    }

    #[test]
    fn stale_journal_detection() {
        assert!(!journal_is_stale(JOURNAL_NONE));
        assert!(journal_is_stale(JOURNAL_INSTALL));
        assert!(journal_is_stale(JOURNAL_UPDATE));
        assert!(journal_is_stale(JOURNAL_REMOVE));
        assert!(journal_is_stale(JOURNAL_ROLLBACK));
    }

    #[test]
    fn journal_action_names_match_codes() {
        assert_eq!(journal_action_name(JOURNAL_NONE), "none");
        assert_eq!(journal_action_name(JOURNAL_UPDATE), "update");
        assert_eq!(journal_action_name(99), "none");
    }

    #[test]
    fn version_argument_splits_source_suffix() {
        assert_eq!(split_version_source("1.4.0"), ("1.4.0", None));
        assert_eq!(split_version_source("1.4.0@beta"), ("1.4.0", Some("beta")));
        assert_eq!(split_version_source("@beta"), ("", Some("beta")));
        assert_eq!(
            split_version_source("1.4.0@community-repo"),
            ("1.4.0", Some("community-repo"))
        );
        // A lone '@' carries no source information.
        assert_eq!(split_version_source("1.4.0@"), ("1.4.0", None));
        assert_eq!(split_version_source(" 1.4.0 "), ("1.4.0", None));
    }

    #[test]
    fn source_restriction_filters_candidates() {
        assert!(source_permits(None, 2, true));
        assert!(source_permits(Some(1), 1, true));
        assert!(!source_permits(Some(1), 2, true));
        assert!(!source_permits(Some(1), 1, false));
    }
}
