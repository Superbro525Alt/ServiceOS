use crate::types::DirtyState;
use serviceos_userspace_runtime as rt;

/// Client fence-wait control-op tags. Kept service-local (same policy as the
/// `0x910` output-create pair) so the shared ABI stays untouched; values sit
/// in the unallocated remainder of the `0x910` graphics tag range and are
/// additive by construction.
pub(crate) const FENCE_WAIT_REQUEST_TAG: u32 = 0x912;
pub(crate) const FENCE_WAIT_REPLY_TAG: u32 = 0x913;

/// Parked client fence-wait requests are bounded: a full waiter list rejects
/// further waits with `CapacityExceeded` instead of growing without limit.
pub(crate) const MAX_FENCE_WAITERS: usize = 8;

pub(crate) struct FenceTracker {
    next_fence: u64,
    completed_fence: u64,
}

impl FenceTracker {
    pub(crate) const fn new() -> Self {
        Self {
            next_fence: 1,
            completed_fence: 0,
        }
    }

    pub(crate) fn issue(&mut self) -> u64 {
        let fence = self.next_fence;
        self.next_fence = self.next_fence.saturating_add(1);
        fence
    }

    pub(crate) fn complete(&mut self, fence: u64) {
        if fence > self.completed_fence {
            self.completed_fence = fence;
        }
    }

    pub(crate) fn completed(&self) -> u64 {
        self.completed_fence
    }
}

pub(crate) fn pending_frame_budget(dirty: &DirtyState) -> u64 {
    match *dirty {
        DirtyState::Clean => 1,
        DirtyState::CursorOnly(_) => 1,
        DirtyState::Region { damages, .. } => (damages.len.max(1)) as u64,
        DirtyState::Full { .. } => 1,
    }
}

pub(crate) fn fence_for_request(present_count: u64, dirty: &DirtyState) -> u64 {
    present_count.saturating_add(pending_frame_budget(dirty))
}

/// Decision for a client fence-wait request, derived from pure completion and
/// timeout math so host tests can pin the semantics without IPC.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WaitDecision {
    /// `completed >= token` already holds: reply success immediately.
    AlreadyComplete,
    /// Incomplete with a positive timeout: park until completion or the
    /// saturated deadline tick.
    Park { deadline_tick: u64 },
    /// Incomplete with a zero timeout: bounded poll that answers right away.
    ImmediateTimeout,
}

/// Fence tokens read complete through the completed-fence high-water mark
/// (`completed >= token`), matching the output-status word12 query contract.
pub(crate) fn fence_is_complete(completed: u64, token: u64) -> bool {
    completed >= token
}

/// Deadline math saturates instead of wrapping so a `u64::MAX` now-tick can
/// never make a waiter live forever.
pub(crate) fn wait_deadline(now: u64, timeout_ticks: u64) -> u64 {
    now.saturating_add(timeout_ticks)
}

/// Expiry is inclusive: at `now == deadline_tick` the wait has timed out.
pub(crate) fn wait_expired(now: u64, deadline_tick: u64) -> bool {
    now >= deadline_tick
}

pub(crate) fn decide_fence_wait(
    completed: u64,
    token: u64,
    now: u64,
    timeout_ticks: u64,
) -> WaitDecision {
    if fence_is_complete(completed, token) {
        return WaitDecision::AlreadyComplete;
    }
    if timeout_ticks == 0 {
        return WaitDecision::ImmediateTimeout;
    }
    WaitDecision::Park {
        deadline_tick: wait_deadline(now, timeout_ticks),
    }
}

#[derive(Clone, Copy)]
pub(crate) struct FenceWaiter {
    pub(crate) reply_handle: rt::Handle,
    pub(crate) token: u64,
    pub(crate) deadline_tick: u64,
}

/// A reaped waiter plus why it left the list; the service turns this into a
/// reply on `reply_handle`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReapedWait {
    Completed(rt::Handle),
    TimedOut(rt::Handle),
}

impl ReapedWait {
    pub(crate) fn handle(&self) -> rt::Handle {
        match *self {
            ReapedWait::Completed(handle) | ReapedWait::TimedOut(handle) => handle,
        }
    }

    pub(crate) fn completed(&self) -> bool {
        matches!(self, ReapedWait::Completed(_))
    }
}

#[derive(Clone, Copy)]
pub(crate) struct FenceWaiters {
    slots: [Option<FenceWaiter>; MAX_FENCE_WAITERS],
}

impl FenceWaiters {
    pub(crate) const fn new() -> Self {
        Self {
            slots: [None; MAX_FENCE_WAITERS],
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.slots.iter().filter(|slot| slot.is_some()).count()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Park a waiter. When the list is full the reply handle is handed back
    /// (`Err`) so the caller can answer `CapacityExceeded` and close it.
    pub(crate) fn park(&mut self, waiter: FenceWaiter) -> Result<(), rt::Handle> {
        if let Some(slot) = self.slots.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(waiter);
            return Ok(());
        }
        Err(waiter.reply_handle)
    }

    /// Earliest pending deadline, for sizing the main loop's receive timeout.
    pub(crate) fn earliest_deadline(&self) -> Option<u64> {
        self.slots
            .iter()
            .filter_map(|slot| slot.as_ref())
            .map(|waiter| waiter.deadline_tick)
            .min()
    }

    /// Remove every waiter satisfied by the completed high-water mark or
    /// expired by `now`, returning them in slot order for reply dispatch.
    /// Still-pending waiters stay parked.
    pub(crate) fn reap(
        &mut self,
        completed: u64,
        now: u64,
        out: &mut [ReapedWait; MAX_FENCE_WAITERS],
    ) -> usize {
        let mut written = 0usize;
        for slot in self.slots.iter_mut() {
            let Some(waiter) = *slot else {
                continue;
            };
            let done = fence_is_complete(completed, waiter.token);
            let expired = !done && wait_expired(now, waiter.deadline_tick);
            if !done && !expired {
                continue;
            }
            out[written] = if done {
                ReapedWait::Completed(waiter.reply_handle)
            } else {
                ReapedWait::TimedOut(waiter.reply_handle)
            };
            written += 1;
            *slot = None;
        }
        written
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{DamageRect, DamageSet};

    #[test]
    fn issue_assigns_monotonic_frame_counters_starting_at_one() {
        let mut tracker = FenceTracker::new();
        assert_eq!(tracker.issue(), 1);
        assert_eq!(tracker.issue(), 2);
        assert_eq!(tracker.issue(), 3);
    }

    #[test]
    fn completion_requires_explicit_complete() {
        let mut tracker = FenceTracker::new();
        let first = tracker.issue();
        assert!(
            first > tracker.completed(),
            "uncompleted fence must read as pending: {first} vs {}",
            tracker.completed()
        );
        tracker.complete(first);
        assert!(
            tracker.completed() >= first,
            "client query form: fence complete iff completed() >= token"
        );
    }

    #[test]
    fn client_query_semantics_completed_high_water_mark() {
        let mut tracker = FenceTracker::new();
        let first = tracker.issue();
        let second = tracker.issue();
        assert!(first > tracker.completed());
        tracker.complete(second);
        // A client asking "is fence F done?" compares F to the completed
        // counter; both outstanding tokens now read complete.
        assert!(tracker.completed() >= second);
        assert!(tracker.completed() >= first);
        tracker.complete(first);
        assert_eq!(tracker.completed(), second);
    }

    #[test]
    fn zero_fence_counts_as_already_complete() {
        let tracker = FenceTracker::new();
        assert!(tracker.completed() >= 0);
    }

    #[test]
    fn saturating_issue_never_wraps() {
        let mut tracker = FenceTracker {
            next_fence: u64::MAX,
            completed_fence: u64::MAX - 2,
        };
        assert_eq!(tracker.issue(), u64::MAX);
        assert_eq!(tracker.issue(), u64::MAX);
        assert_eq!(tracker.completed(), u64::MAX - 2);
    }

    fn rect(x: i32, y: i32, width: u32, height: u32) -> DamageRect {
        DamageRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn budget_counts_region_rect_frames() {
        let mut damages = DamageSet::empty();
        damages = damages.push(rect(0, 0, 4, 4));
        damages = damages.push(rect(10, 10, 4, 4));
        let dirty = DirtyState::Region {
            damages,
            immediate: true,
        };
        assert_eq!(pending_frame_budget(&dirty), 2);
    }

    #[test]
    fn budget_is_one_frame_for_full_cursor_and_clean() {
        assert_eq!(pending_frame_budget(&DirtyState::Clean), 1);
        assert_eq!(
            pending_frame_budget(&DirtyState::CursorOnly(rect(1, 1, 2, 2))),
            1
        );
        assert_eq!(
            pending_frame_budget(&DirtyState::Full { immediate: false }),
            1
        );
    }

    #[test]
    fn empty_region_still_budgets_one_frame() {
        let dirty = DirtyState::Region {
            damages: DamageSet::empty(),
            immediate: true,
        };
        assert_eq!(pending_frame_budget(&dirty), 1);
    }

    #[test]
    fn fence_for_request_is_conservative_over_remaining_turn() {
        let mut damages = DamageSet::empty();
        damages = damages.push(rect(0, 0, 4, 4));
        damages = damages.push(rect(10, 10, 8, 8));
        let dirty = DirtyState::Region {
            damages,
            immediate: true,
        };
        assert_eq!(fence_for_request(7, &dirty), 9);
        assert_eq!(fence_for_request(0, &DirtyState::Clean), 1);
    }

    #[test]
    fn wait_already_complete_when_high_water_covers_token() {
        let mut tracker = FenceTracker::new();
        let token = tracker.issue();
        tracker.complete(token);
        assert!(fence_is_complete(tracker.completed(), token));
        assert_eq!(
            decide_fence_wait(tracker.completed(), token, 100, 50),
            WaitDecision::AlreadyComplete
        );
        // Token zero reads complete against an untouched tracker.
        assert_eq!(
            decide_fence_wait(tracker.completed(), 0, 100, 0),
            WaitDecision::AlreadyComplete
        );
    }

    #[test]
    fn wait_parks_with_saturated_deadline_when_incomplete() {
        let tracker = FenceTracker::new();
        let decision = decide_fence_wait(tracker.completed(), 3, 1_000, 250);
        assert_eq!(decision, WaitDecision::Park { deadline_tick: 1_250 });
        // Deadline math saturates instead of wrapping.
        assert_eq!(
            decide_fence_wait(0, u64::MAX - 1, u64::MAX - 10, 100),
            WaitDecision::Park {
                deadline_tick: u64::MAX
            }
        );
    }

    #[test]
    fn wait_zero_timeout_polls_without_parking() {
        let tracker = FenceTracker::new();
        assert_eq!(
            decide_fence_wait(tracker.completed(), 9, 500, 0),
            WaitDecision::ImmediateTimeout
        );
    }

    #[test]
    fn wait_expiry_boundary_is_inclusive() {
        assert!(!wait_expired(99, 100));
        assert!(wait_expired(100, 100));
        assert!(wait_expired(101, 100));
    }

    #[test]
    fn waiters_reap_completes_every_token_below_high_water() {
        let mut waiters = FenceWaiters::new();
        let a = 0x11;
        let b = 0x22;
        assert_eq!(
            waiters.park(FenceWaiter {
                reply_handle: a,
                token: 5,
                deadline_tick: 1_000
            }),
            Ok(())
        );
        assert_eq!(
            waiters.park(FenceWaiter {
                reply_handle: b,
                token: 7,
                deadline_tick: 2_000
            }),
            Ok(())
        );
        let mut out = [ReapedWait::TimedOut(rt::INVALID_HANDLE); MAX_FENCE_WAITERS];
        // High-water 7 completes both parked tokens regardless of deadlines.
        let reaped = waiters.reap(7, 0, &mut out);
        assert_eq!(reaped, 2);
        assert!(out[..reaped].iter().all(ReapedWait::completed));
        assert!(waiters.is_empty());
    }

    #[test]
    fn waiters_reap_times_out_only_past_deadline_and_keeps_pending() {
        let mut waiters = FenceWaiters::new();
        let expired_handle = 0x33;
        let live_handle = 0x44;
        waiters
            .park(FenceWaiter {
                reply_handle: expired_handle,
                token: 9,
                deadline_tick: 500,
            })
            .expect("empty list parks");
        waiters
            .park(FenceWaiter {
                reply_handle: live_handle,
                token: 9,
                deadline_tick: 5_000,
            })
            .expect("space remains");
        let mut out = [ReapedWait::TimedOut(rt::INVALID_HANDLE); MAX_FENCE_WAITERS];
        let reaped = waiters.reap(0, 500, &mut out);
        assert_eq!(reaped, 1);
        assert!(!out[0].completed());
        assert_eq!(out[0].handle(), expired_handle);
        assert_eq!(waiters.len(), 1);
        assert_eq!(waiters.earliest_deadline(), Some(5_000));
    }

    #[test]
    fn waiters_full_list_hands_reply_handle_back() {
        let mut waiters = FenceWaiters::new();
        for index in 0..MAX_FENCE_WAITERS {
            waiters
                .park(FenceWaiter {
                    reply_handle: 100 + index as rt::Handle,
                    token: 1,
                    deadline_tick: 10,
                })
                .expect("list has room until capacity");
        }
        assert_eq!(waiters.len(), MAX_FENCE_WAITERS);
        let overflow = FenceWaiter {
            reply_handle: 0x77,
            token: 1,
            deadline_tick: 10,
        };
        assert_eq!(waiters.park(overflow), Err(0x77));
        assert_eq!(waiters.earliest_deadline(), Some(10));
    }
}
