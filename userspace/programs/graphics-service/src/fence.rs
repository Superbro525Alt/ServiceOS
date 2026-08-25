use crate::types::DirtyState;

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
}
