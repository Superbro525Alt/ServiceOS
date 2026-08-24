pub(crate) const TIMELINE_CAP: usize = 64;
pub(crate) const TIMELINE_SERVICES_CAP: usize = 24;
pub(crate) const TIMELINE_REPLY_EVENTS: usize = 5;

/// Local protocol tags (ABI `StatusTag` ends at `0x409`).
/// Query request words: `[mode, arg]`, handle 0 = reply channel; mode
/// `QUERY_MODE_SINCE` filters `tick >= arg`, any other value means "last N"
/// where N is `arg`. Reply: `[count, (id|kind<<32), tick, from|to<<32] * count`.
pub(crate) mod timeline_tag {
    pub const QUERY_REQUEST: u32 = 0x410;
    pub const QUERY_REPLY: u32 = 0x411;
    pub const SUMMARY_REQUEST: u32 = 0x412;
    pub const SUMMARY_REPLY: u32 = 0x413;

    pub const QUERY_MODE_SINCE: u64 = 1;
}

pub(crate) mod event_kind {
    pub const SEED: u32 = 0;
    pub const STATE_CHANGE: u32 = 1;
    pub const RESTART: u32 = 2;
    pub const CRASH: u32 = 3;
    pub const HEALTH_FLIP: u32 = 4;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TimelineEvent {
    pub service_id: u32,
    pub kind: u32,
    pub tick: u64,
    pub from: u32,
    pub to: u32,
}

impl TimelineEvent {
    pub(crate) const fn zeroed() -> Self {
        Self {
            service_id: 0,
            kind: 0,
            tick: 0,
            from: 0,
            to: 0,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Timeline {
    buf: [TimelineEvent; TIMELINE_CAP],
    head: usize,
    len: usize,
    pushed: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TimelineSummary {
    pub retained: usize,
    pub pushed: u64,
    pub per_service: [(u32, usize); TIMELINE_SERVICES_CAP],
    pub per_service_len: usize,
    pub first_tick: Option<u64>,
    pub last_tick: Option<u64>,
    pub busiest_service: u32,
    pub busiest_count: usize,
}

impl TimelineSummary {
    pub(crate) const fn empty() -> Self {
        Self {
            retained: 0,
            pushed: 0,
            per_service: [(0, 0); TIMELINE_SERVICES_CAP],
            per_service_len: 0,
            first_tick: None,
            last_tick: None,
            busiest_service: 0,
            busiest_count: 0,
        }
    }
}

impl Timeline {
    pub(crate) const fn new() -> Self {
        Self {
            buf: [TimelineEvent::zeroed(); TIMELINE_CAP],
            head: 0,
            len: 0,
            pushed: 0,
        }
    }

    pub(crate) fn push(&mut self, event: TimelineEvent) {
        self.buf[self.head] = event;
        self.head = (self.head + 1) % TIMELINE_CAP;
        if self.len < TIMELINE_CAP {
            self.len += 1;
        }
        self.pushed = self.pushed.saturating_add(1);
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn total_pushed(&self) -> u64 {
        self.pushed
    }

    /// Fills `out` with up to `out.len()` newest events, oldest first.
    pub(crate) fn query_last(&self, max: usize, out: &mut [TimelineEvent]) -> usize {
        let take = max.min(out.len()).min(self.len);
        let start_visible = self.len - take;
        for slot in 0..take {
            let index =
                (self.head + self.len_cap() - self.len + start_visible + slot) % TIMELINE_CAP;
            out[slot] = self.buf[index];
        }
        take
    }

    /// Fills `out` with retained events whose tick is `>= since_tick`,
    /// in push order. Seeded ticks may predate live monotonic ticks.
    pub(crate) fn query_since(&self, since_tick: u64, out: &mut [TimelineEvent]) -> usize {
        let mut count = 0usize;
        for event in self.iter_oldest_first() {
            if event.tick < since_tick {
                continue;
            }
            if count >= out.len() {
                break;
            }
            out[count] = event;
            count += 1;
        }
        count
    }

    fn len_cap(&self) -> usize {
        TIMELINE_CAP
    }

    fn iter_oldest_first(&self) -> impl Iterator<Item = TimelineEvent> + '_ {
        let oldest = (self.head + TIMELINE_CAP - self.len) % TIMELINE_CAP;
        (0..self.len).map(move |slot| self.buf[(oldest + slot) % TIMELINE_CAP])
    }
}

pub(crate) fn compute_timeline_summary(timeline: &Timeline) -> TimelineSummary {
    let mut summary = TimelineSummary::empty();
    summary.retained = timeline.len();
    summary.pushed = timeline.total_pushed();

    let mut first_tick = None;
    let mut last_tick = None;
    for event in timeline.iter_oldest_first() {
        if first_tick.is_none() {
            first_tick = Some(event.tick);
        }
        last_tick = Some(event.tick);

        let service_id = event.service_id;
        let existing = summary.per_service[..summary.per_service_len]
            .iter_mut()
            .find(|(id, _)| *id == service_id);
        match existing {
            Some((_, count)) => *count += 1,
            None if summary.per_service_len < TIMELINE_SERVICES_CAP => {
                summary.per_service[summary.per_service_len] = (service_id, 1);
                summary.per_service_len += 1;
            }
            None => {}
        }
    }
    summary.first_tick = first_tick;
    summary.last_tick = last_tick;

    for index in 0..summary.per_service_len {
        let (service_id, count) = summary.per_service[index];
        if count > summary.busiest_count
            || (count == summary.busiest_count
                && busiest_better(service_id, summary.busiest_service))
        {
            summary.busiest_service = service_id;
            summary.busiest_count = count;
        }
    }
    summary
}

fn busiest_better(candidate: u32, incumbent: u32) -> bool {
    incumbent == 0 || candidate < incumbent
}

#[cfg(test)]
mod tests {
    use super::*;

    const SID_A: u32 = 3;
    const SID_B: u32 = 5;

    fn ev(service_id: u32, kind: u32, tick: u64) -> TimelineEvent {
        TimelineEvent {
            service_id,
            kind,
            tick,
            from: 0,
            to: 0,
        }
    }

    #[test]
    fn ring_retains_bounded_window_dropping_oldest() {
        let mut timeline = Timeline::new();
        let extra = 10u64;
        for tick in 0..TIMELINE_CAP as u64 + extra {
            timeline.push(ev(SID_A, event_kind::STATE_CHANGE, tick));
        }
        assert_eq!(timeline.len(), TIMELINE_CAP);
        assert_eq!(timeline.total_pushed(), TIMELINE_CAP as u64 + extra);

        let mut out = [TimelineEvent::zeroed(); TIMELINE_CAP];
        let count = timeline.query_last(TIMELINE_CAP, &mut out);
        assert_eq!(count, TIMELINE_CAP);
        assert_eq!(out[0].tick, extra);
        assert_eq!(out[TIMELINE_CAP - 1].tick, TIMELINE_CAP as u64 + extra - 1);
    }

    #[test]
    fn query_last_returns_newest_events_in_push_order() {
        let mut timeline = Timeline::new();
        for tick in 0..8u64 {
            timeline.push(ev(SID_A, event_kind::RESTART, tick));
        }
        let mut out = [TimelineEvent::zeroed(); 8];
        let count = timeline.query_last(3, &mut out);
        assert_eq!(count, 3);
        assert_eq!(out[0].tick, 5);
        assert_eq!(out[1].tick, 6);
        assert_eq!(out[2].tick, 7);
    }

    #[test]
    fn query_since_filters_by_monotonic_tick_keeping_order() {
        let mut timeline = Timeline::new();
        for tick in [10u64, 20, 30] {
            timeline.push(ev(SID_B, event_kind::HEALTH_FLIP, tick));
        }
        let mut out = [TimelineEvent::zeroed(); TIMELINE_CAP];
        let count = timeline.query_since(20, &mut out);
        assert_eq!(count, 2);
        assert_eq!(out[0].tick, 20);
        assert_eq!(out[1].tick, 30);

        let count = timeline.query_since(31, &mut out);
        assert_eq!(count, 0);
    }

    #[test]
    fn summary_counts_first_last_and_busiest_service() {
        let mut timeline = Timeline::new();
        timeline.push(ev(SID_A, event_kind::SEED, 100));
        timeline.push(ev(SID_B, event_kind::SEED, 110));
        timeline.push(ev(SID_A, event_kind::CRASH, 120));
        timeline.push(ev(SID_A, event_kind::RESTART, 130));
        timeline.push(ev(SID_B, event_kind::STATE_CHANGE, 140));

        let summary = compute_timeline_summary(&timeline);
        assert_eq!(summary.retained, 5);
        assert_eq!(summary.pushed, 5);
        assert_eq!(summary.first_tick, Some(100));
        assert_eq!(summary.last_tick, Some(140));
        assert_eq!(summary.busiest_service, SID_A);
        assert_eq!(summary.busiest_count, 3);
        assert_eq!(summary.per_service_len, 2);
    }

    #[test]
    fn summary_busiest_tie_prefers_lowest_service_id() {
        let mut timeline = Timeline::new();
        timeline.push(ev(SID_B, event_kind::SEED, 1));
        timeline.push(ev(SID_A, event_kind::SEED, 2));
        timeline.push(ev(SID_B, event_kind::SEED, 3));
        timeline.push(ev(SID_A, event_kind::SEED, 4));

        let summary = compute_timeline_summary(&timeline);
        assert_eq!(summary.busiest_service, SID_A);
        assert_eq!(summary.busiest_count, 2);
    }

    #[test]
    fn summary_empty_timeline_is_zeroed() {
        let summary = compute_timeline_summary(&Timeline::new());
        assert_eq!(summary, TimelineSummary::empty());
    }
}
