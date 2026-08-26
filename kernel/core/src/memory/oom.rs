use crate::task::TaskId;
use alloc::vec::Vec;
use spin::Mutex;

/// Distinct fault-style exit reason carried by tasks terminated by the OOM
/// policy (`TaskExitStatus::Faulted { code: OOM_EXIT_CODE }`). The ASCII
/// "OOM" tag keeps it recognizable in raw exit-word dumps.
pub const OOM_EXIT_CODE: u64 = 0x0000_4F4F_4D00;

/// Tasks the OOM policy must never select as victims. Matched against the
/// candidate's service name when one is known.
pub const PROTECTED_TASK_NAMES: [&str; 3] = ["root-manager", "console-service", "shell-service"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VictimCandidate {
    pub task: TaskId,
    pub name: Option<&'static str>,
    pub footprint_frames: u64,
    pub reclaimable: bool,
}

/// A candidate is protected when it is not marked reclaimable or carries a
/// protected service name.
pub fn is_protected(candidate: &VictimCandidate) -> bool {
    !candidate.reclaimable
        || candidate
            .name
            .is_some_and(|name| PROTECTED_TASK_NAMES.contains(&name))
}

/// Pick the OOM victim: the largest-footprint reclaimable task that is not
/// protected. Ties break toward the lowest task id for determinism.
pub fn select_victim(candidates: &[VictimCandidate]) -> Option<VictimCandidate> {
    candidates
        .iter()
        .copied()
        .filter(|candidate| !is_protected(candidate))
        .max_by(|a, b| {
            a.footprint_frames
                .cmp(&b.footprint_frames)
                .then_with(|| b.task.cmp(&a.task))
        })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OomError {
    /// No recovery hooks are installed, so no victim can even be found.
    NoRecoveryAvailable,
    /// Every candidate was protected (or none existed): allocation cannot
    /// be satisfied by reclamation and the caller must panic.
    ProtectedSetExhausted,
}

/// Kernel-side recovery hooks. `candidates` enumerates live task memory
/// usage; `reclaim` terminates the chosen victim with the distinct OOM exit
/// reason and returns its frames to the allocator.
pub struct OomHooks {
    pub candidates: fn() -> Vec<VictimCandidate>,
    pub reclaim: fn(VictimCandidate),
}

static OOM_HOOKS: Mutex<Option<OomHooks>> = Mutex::new(None);

pub fn register_oom_hooks(hooks: OomHooks) {
    *OOM_HOOKS.lock() = Some(hooks);
}

#[cfg(test)]
pub fn clear_oom_hooks_for_tests() {
    *OOM_HOOKS.lock() = None;
}

/// Retry-once OOM driver for an allocation that already failed once:
/// reclaim exactly one victim, then retry allocation exactly once.
/// `ProtectedSetExhausted` when no eligible victim exists or the retry
/// still fails; `NoRecoveryAvailable` when no hooks are installed.
pub fn recover_with_retry<T>(mut allocate: impl FnMut() -> Option<T>) -> Result<T, OomError> {
    let (candidates, reclaim) = {
        let hooks = OOM_HOOKS.lock();
        match hooks.as_ref() {
            Some(hooks) => (hooks.candidates, hooks.reclaim),
            None => return Err(OomError::NoRecoveryAvailable),
        }
    };

    let victim = select_victim(&candidates()).ok_or(OomError::ProtectedSetExhausted)?;
    reclaim(victim);

    allocate().ok_or(OomError::ProtectedSetExhausted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn victim(id: u64, footprint: u64) -> VictimCandidate {
        VictimCandidate {
            task: TaskId(id),
            name: None,
            footprint_frames: footprint,
            reclaimable: true,
        }
    }

    static RECLAIMED: Mutex<Vec<u64>> = Mutex::new(Vec::new());

    fn record_reclaim(candidate: VictimCandidate) {
        RECLAIMED.lock().push(candidate.task.0);
    }

    #[test]
    fn victim_selection_matrix_including_protected_set() {
        // Empty candidate list has no victim.
        assert_eq!(select_victim(&[]), None);

        // Largest reclaimable footprint wins.
        let candidates = [victim(1, 10), victim(2, 400), victim(3, 120)];
        assert_eq!(select_victim(&candidates), Some(victim(2, 400)));

        // Ties break to the lowest task id.
        let tied = [victim(9, 100), victim(4, 100), victim(7, 100)];
        assert_eq!(select_victim(&tied), Some(victim(4, 100)));

        // Not-marked-reclaimable candidates are protected even when largest.
        let mut unmarked = victim(5, 999);
        unmarked.reclaimable = false;
        let candidates = [unmarked, victim(6, 3)];
        assert_eq!(select_victim(&candidates), Some(victim(6, 3)));

        // Known protected service names are never selected.
        let mut root_manager = victim(8, 5000);
        root_manager.name = Some("root-manager");
        let mut console = victim(9, 4000);
        console.name = Some("console-service");
        let mut shell = victim(10, 3000);
        shell.name = Some("shell-service");
        let candidates = [root_manager, console, shell, victim(11, 1)];
        assert_eq!(select_victim(&candidates), Some(victim(11, 1)));

        // Protection wins over the reclaimable flag alone.
        assert!(is_protected(&root_manager));
        assert!(!is_protected(&victim(12, 7)));

        // All-protected matrix exhausts without a victim.
        let all_protected = [root_manager, console, shell, unmarked];
        assert_eq!(select_victim(&all_protected), None);
    }

    #[test]
    fn retry_once_recovery_and_exhaustion_paths() {
        clear_oom_hooks_for_tests();

        // Without hooks there is no recovery path at all.
        let mut attempts = 0u32;
        let outcome: Result<(), OomError> = recover_with_retry(|| {
            attempts += 1;
            None
        });
        assert_eq!(outcome, Err(OomError::NoRecoveryAvailable));
        assert_eq!(attempts, 0, "failed allocation must not be retried blindly");

        register_oom_hooks(OomHooks {
            candidates: || alloc::vec![victim(21, 50), victim(22, 80)],
            reclaim: record_reclaim,
        });

        // Failure -> largest victim reclaimed -> single retry succeeds.
        RECLAIMED.lock().clear();
        let mut retries = 0u32;
        let outcome = recover_with_retry(|| {
            retries += 1;
            Some(())
        });
        assert_eq!(outcome, Ok(()));
        assert_eq!(retries, 1, "exactly one retry after reclamation");
        assert_eq!(
            RECLAIMED.lock().as_slice(),
            &[22],
            "largest-footprint victim reclaimed"
        );

        // Retry still failing after the single permitted reclaim exhausts.
        RECLAIMED.lock().clear();
        let outcome = recover_with_retry(|| None::<()>);
        assert_eq!(outcome, Err(OomError::ProtectedSetExhausted));
        assert_eq!(RECLAIMED.lock().len(), 1);

        // Protected-set exhaustion with zero eligible candidates.
        RECLAIMED.lock().clear();
        clear_oom_hooks_for_tests();
        register_oom_hooks(OomHooks {
            candidates: || alloc::vec![],
            reclaim: record_reclaim,
        });
        let outcome = recover_with_retry(|| Some(()));
        assert_eq!(outcome, Err(OomError::ProtectedSetExhausted));
        assert!(RECLAIMED.lock().is_empty());
        clear_oom_hooks_for_tests();
    }
}
