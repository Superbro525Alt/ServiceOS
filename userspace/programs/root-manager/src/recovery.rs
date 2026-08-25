use rt::ServiceId;
use serviceos_bundle::RestartPolicy;

use serviceos_userspace_runtime as rt;

/// Crash-loop escalation window: N crashes within this many ticks escalates
/// the service to fail-stop regardless of its configured restart policy.
pub(crate) const CRASH_WINDOW_TICKS: u64 = 1000;
pub(crate) const CRASH_LOOP_LIMIT: usize = 3;

/// Ring of the most recent crash ticks for one service slot.
#[derive(Clone, Copy)]
pub(crate) struct CrashWindow {
    ticks: [u64; CRASH_LOOP_LIMIT],
    cursor: usize,
    filled: usize,
}

impl CrashWindow {
    pub(crate) const fn new() -> Self {
        Self {
            ticks: [0; CRASH_LOOP_LIMIT],
            cursor: 0,
            filled: 0,
        }
    }

    pub(crate) fn record(&mut self, now: u64) {
        self.ticks[self.cursor] = now;
        self.cursor = (self.cursor + 1) % CRASH_LOOP_LIMIT;
        self.filled = (self.filled + 1).min(CRASH_LOOP_LIMIT);
    }

    /// True once `CRASH_LOOP_LIMIT` recorded crashes all sit inside a
    /// `CRASH_WINDOW_TICKS` span ending at `now`.
    pub(crate) fn should_escalate(&self, now: u64) -> bool {
        if self.filled < CRASH_LOOP_LIMIT {
            return false;
        }
        let oldest = self.ticks.iter().copied().fold(u64::MAX, u64::min);
        now.saturating_sub(oldest) <= CRASH_WINDOW_TICKS
    }

    pub(crate) fn span(&self, now: u64) -> u64 {
        if self.filled == 0 {
            return 0;
        }
        let oldest = self.ticks.iter().copied().fold(u64::MAX, u64::min);
        now.saturating_sub(oldest)
    }
}

pub(crate) const USER_FAULT_EXIT_TAG: u64 = 0xf100_0000_0000_0000;

/// Decoded user-fault exit word. Mirrors the additive kernel packing (tag
/// bits 63..48, address low-32 bits 47..16, class nibble 15..12, legacy
/// detail 11..0); see kernel/core/src/fault.rs and the roadmap S1 row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct UserFaultBits {
    pub(crate) class: u64,
    pub(crate) address: u64,
}

pub(crate) fn decode_user_fault(exit_code: u64) -> Option<UserFaultBits> {
    if exit_code & 0xffff_0000_0000_0000 != USER_FAULT_EXIT_TAG {
        return None;
    }
    Some(UserFaultBits {
        class: (exit_code >> 12) & 0xf,
        address: (exit_code >> 16) & 0xffff_ffff,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExitKind {
    /// Clean zero exit; never consumes restart budget.
    #[allow(dead_code)]
    CleanExit,
    Failure {
        user_fault: bool,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RecoveryDecision {
    /// Schedule a restart after `backoff_base` ticks (exponential scaling is
    /// applied by the caller from consecutive-failure count).
    Restart { backoff_base: u32 },
    /// Notify `supervisor` with fault details first, then behave as Restart.
    SupervisorCall {
        supervisor: ServiceId,
        backoff_base: u32,
    },
    /// No restart: leave the task marked failed for the operator.
    FailStop,
}

/// Policy decision matrix. Escalation (crash loop) overrides every policy.
/// Clean exits never consume restart budget; failures do once
/// `failures >= max_restarts` unless an operator explicitly re-requested a
/// start (`requested_restart`).
pub(crate) fn decide_recovery(
    policy: RestartPolicy,
    kind: ExitKind,
    escalated: bool,
    failures: u32,
    requested_restart: bool,
) -> RecoveryDecision {
    if escalated {
        return RecoveryDecision::FailStop;
    }
    let budget_exhausted = matches!(kind, ExitKind::Failure { .. })
        && !requested_restart
        && failures >= max_restarts_of(policy);
    match policy {
        RestartPolicy::FailStop => RecoveryDecision::FailStop,
        RestartPolicy::OnFailure { backoff_ticks, .. } => {
            if budget_exhausted {
                RecoveryDecision::FailStop
            } else {
                RecoveryDecision::Restart {
                    backoff_base: backoff_ticks,
                }
            }
        }
        RestartPolicy::SupervisorRestart {
            supervisor,
            backoff_ticks,
            ..
        } => {
            if budget_exhausted {
                RecoveryDecision::FailStop
            } else {
                RecoveryDecision::SupervisorCall {
                    supervisor,
                    backoff_base: backoff_ticks,
                }
            }
        }
    }
}

fn max_restarts_of(policy: RestartPolicy) -> u32 {
    match policy {
        RestartPolicy::FailStop => 0,
        RestartPolicy::OnFailure { max_restarts, .. }
        | RestartPolicy::SupervisorRestart { max_restarts, .. } => max_restarts,
    }
}

/// Base backoff ticks for any restart-capable policy (0 allowed).
pub(crate) fn base_backoff(policy: RestartPolicy) -> u32 {
    match policy {
        RestartPolicy::FailStop => 0,
        RestartPolicy::OnFailure { backoff_ticks, .. }
        | RestartPolicy::SupervisorRestart { backoff_ticks, .. } => backoff_ticks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ON_FAILURE: RestartPolicy = RestartPolicy::OnFailure {
        max_restarts: 2,
        backoff_ticks: 10,
    };
    const FAIL_STOP: RestartPolicy = RestartPolicy::FailStop;
    const SUPERVISOR: RestartPolicy = RestartPolicy::SupervisorRestart {
        supervisor: ServiceId::Status,
        max_restarts: 1,
        backoff_ticks: 5,
    };

    #[test]
    fn crash_window_escalates_on_three_crashes_inside_span() {
        let mut window = CrashWindow::new();
        assert!(!window.should_escalate(500));
        window.record(10);
        assert!(!window.should_escalate(20));
        window.record(20);
        window.record(30);
        assert!(window.should_escalate(30));
        assert_eq!(window.span(30), 20);

        // Ring wraparound keeps the oldest entry honest.
        let mut wrapped = CrashWindow::new();
        wrapped.record(5);
        wrapped.record(2000);
        wrapped.record(4000);
        // Oldest (5) fell out of a 1000-tick span ending at 4000.
        assert!(!wrapped.should_escalate(4000));

        // Crashes spread beyond the window do not escalate.
        let mut spread = CrashWindow::new();
        spread.record(0);
        spread.record(1500);
        spread.record(3000);
        assert!(!spread.should_escalate(3000));
    }

    #[test]
    fn user_fault_decode_matches_kernel_packing() {
        let packed: u64 =
            0xf100_0000_0000_0000 | (0xdead_beefu64 << 16) | (3 << 12) | (0x100 | 0x2);
        let bits = decode_user_fault(packed).expect("user fault word");
        assert_eq!(bits.class, 3);
        assert_eq!(bits.address, 0xdead_beef);
        assert!(decode_user_fault(0).is_none());
        assert!(decode_user_fault(0xf670).is_none());
    }

    #[test]
    fn policy_decision_matrix_covers_all_shapes() {
        let failure = ExitKind::Failure { user_fault: true };
        let clean = ExitKind::CleanExit;

        // Escalation wins over everything.
        for policy in [ON_FAILURE, FAIL_STOP, SUPERVISOR] {
            assert_eq!(
                decide_recovery(policy, failure, true, 0, false),
                RecoveryDecision::FailStop
            );
        }

        assert_eq!(
            decide_recovery(FAIL_STOP, failure, false, 0, false),
            RecoveryDecision::FailStop
        );

        assert_eq!(
            decide_recovery(ON_FAILURE, failure, false, 0, false),
            RecoveryDecision::Restart { backoff_base: 10 }
        );
        assert_eq!(
            decide_recovery(ON_FAILURE, failure, false, 2, false),
            RecoveryDecision::FailStop,
            "budget exhausted at max_restarts"
        );
        assert_eq!(
            decide_recovery(ON_FAILURE, failure, false, 5, true),
            RecoveryDecision::Restart { backoff_base: 10 },
            "operator-requested restart ignores budget"
        );
        assert_eq!(
            decide_recovery(ON_FAILURE, clean, false, 9, false),
            RecoveryDecision::Restart { backoff_base: 10 },
            "clean exits never count against the restart budget"
        );

        assert_eq!(
            decide_recovery(SUPERVISOR, failure, false, 0, false),
            RecoveryDecision::SupervisorCall {
                supervisor: ServiceId::Status,
                backoff_base: 5
            }
        );
        assert_eq!(
            decide_recovery(SUPERVISOR, failure, false, 1, false),
            RecoveryDecision::FailStop
        );

        // Zero-budget on-failure is fail-stop from the start.
        let zero = RestartPolicy::OnFailure {
            max_restarts: 0,
            backoff_ticks: 0,
        };
        assert_eq!(
            decide_recovery(zero, failure, false, 0, false),
            RecoveryDecision::FailStop
        );
        assert_eq!(base_backoff(SUPERVISOR), 5);
    }
}
