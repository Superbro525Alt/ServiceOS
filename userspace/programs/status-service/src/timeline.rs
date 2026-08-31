pub(crate) const TIMELINE_CAP: usize = 64;
pub(crate) const TIMELINE_SERVICES_CAP: usize = 24;
pub(crate) const TIMELINE_REPLY_EVENTS: usize = 5;

use serviceos_userspace_runtime::{LogDomain, LogEvent, LogSeverity};

/// Local protocol tags (ABI `StatusTag` ends at `0x409`).
/// Query request words: `[mode, arg...]`, handle 0 = reply channel; mode
/// `QUERY_MODE_SINCE` filters `tick >= arg`, mode `QUERY_MODE_SERVICE`
/// drills down with `[service_id|ANY, kind|ANY, skip]` filters, mode
/// `QUERY_MODE_SERVICE_STATS` takes `[service_id]` and answers on the
/// stats tag, any other value means "last N" where N is `arg`.
/// Reply: `[count, (id|kind<<32), tick, from|to<<32] * count` where the
/// drilldown reply packs the total window matches into `count`'s high half.
pub(crate) mod timeline_tag {
    pub const QUERY_REQUEST: u32 = 0x410;
    pub const QUERY_REPLY: u32 = 0x411;
    pub const SUMMARY_REQUEST: u32 = 0x412;
    pub const SUMMARY_REPLY: u32 = 0x413;
    pub const STATS_REPLY: u32 = 0x415;

    pub const QUERY_MODE_SINCE: u64 = 1;
    pub const QUERY_MODE_SERVICE: u64 = 2;
    pub const QUERY_MODE_SERVICE_STATS: u64 = 3;
}

/// Drilldown filter sentinel: a request word of this value means "do not
/// constrain by that field". Real service ids and event kinds stay below it.
pub(crate) const FILTER_ANY: u32 = u32::MAX;

/// Distinct per-kind buckets kept in a [`ServiceStats`] line. Covers all
/// nine defined kinds plus slack for unknown kinds; overflow kinds are
/// dropped like over-cap services in the summary.
pub(crate) const SERVICE_KINDS_CAP: usize = 12;

pub(crate) mod event_kind {
    pub const SEED: u32 = 0;
    pub const STATE_CHANGE: u32 = 1;
    pub const RESTART: u32 = 2;
    pub const CRASH: u32 = 3;
    pub const HEALTH_FLIP: u32 = 4;
    pub const JOB_PHASE: u32 = 5;
    pub const FRAME_PACING: u32 = 6;
    pub const NET_SELFTEST: u32 = 7;
    pub const PRESSURE: u32 = 8;
}

/// Memory-pressure level discriminants carried in pressure domain-event
/// `from`/`to` fields; mirrors the kernel `PressureLevel` ordering in
/// `kernel/core/src/memory/pressure.rs`.
pub(crate) mod pressure_level {
    pub const NORMAL: u32 = 0;
    pub const TIGHT: u32 = 1;
    pub const CRITICAL: u32 = 2;
}

/// Network selftest phase codes carried in domain-event `to` fields; mirrors
/// the `selftest_phase` module in `network-service/src/protocol/selftest.rs`.
/// The `arg0` tag below discriminates selftest records from operator ping
/// records sharing the `NetworkProbeCompleted` event (pings carry an IPv4
/// address word in `arg0`, always below `0x1_0000_0000`).
pub(crate) mod net_selftest {
    pub const ARG0_TAG: u64 = 0x5345_4C46; // "SELF"
    #[allow(dead_code)]
    pub const BEGIN: u64 = 0;
    pub const PASSED: u64 = 1;
    pub const FAILED: u64 = 2;
}

/// Developer job final-state encodings carried in domain-event `to` fields;
/// mirrors `DeveloperJobState` discriminants in `shared/abi/src/developer.rs`.
pub(crate) mod job_state {
    #[allow(dead_code)]
    pub const QUEUED: u32 = 1;
    pub const RUNNING: u32 = 2;
    pub const SUCCEEDED: u32 = 3;
    pub const FAILED: u32 = 4;
    pub const UNSUPPORTED: u32 = 5;
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

/// Per-domain counters aggregated from ingested domain events. Deliberately
/// independent of the ring window so totals survive event eviction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DomainCounters {
    pub(crate) jobs_started: u64,
    pub(crate) jobs_succeeded: u64,
    pub(crate) jobs_failed: u64,
    pub(crate) long_frames: u64,
    pub(crate) net_selftests_passed: u64,
    pub(crate) net_selftests_failed: u64,
    /// Latest memory-pressure level seen in a transition (0 normal,
    /// 1 tight, 2 critical); starts Normal before any transition arrives.
    pub(crate) pressure_level: u32,
    pub(crate) pressure_transitions: u64,
}

impl DomainCounters {
    pub(crate) const fn empty() -> Self {
        Self {
            jobs_started: 0,
            jobs_succeeded: 0,
            jobs_failed: 0,
            long_frames: 0,
            net_selftests_passed: 0,
            net_selftests_failed: 0,
            pressure_level: pressure_level::NORMAL,
            pressure_transitions: 0,
        }
    }

    /// Packs into the extended SUMMARY_REPLY layout words `[12..16]`. The
    /// kernel IPC envelope hard-caps messages at `IPC_MAX_WORDS` (16), so the
    /// eight counters compress into four words of u32 halves (saturating):
    /// `[12]` jobs started|succeeded, `[13]` jobs failed|long frames,
    /// `[14]` net passed|failed, `[15]` pressure level|transitions.
    pub(crate) fn pack_reply_words(&self) -> [u64; 8] {
        [
            self.jobs_started,
            self.jobs_succeeded,
            self.jobs_failed,
            self.long_frames,
            self.net_selftests_passed,
            self.net_selftests_failed,
            self.pressure_level as u64,
            self.pressure_transitions,
        ]
    }

    /// Wire form of [`Self::pack_reply_words`] that fits the 16-word IPC
    /// envelope: pairs of counters squeezed into u32 halves, oldest first.
    pub(crate) fn pack_reply_words_compact(&self) -> [u64; 4] {
        let half = |value: u64| value.min(u32::MAX as u64);
        [
            half(self.jobs_started) | (half(self.jobs_succeeded) << 32),
            half(self.jobs_failed) | (half(self.long_frames) << 32),
            half(self.net_selftests_passed) | (half(self.net_selftests_failed) << 32),
            self.pressure_level as u64 | (half(self.pressure_transitions) << 32),
        ]
    }
}

/// Ingests one domain event classified from the log stream: records it in
/// the ring like any other timeline event and tallies per-domain counters by
/// final state. Unknown kinds still enter the ring but stay uncounted.
pub(crate) fn ingest_domain_event(
    timeline: &mut Timeline,
    counters: &mut DomainCounters,
    service_id: u32,
    kind: u32,
    tick: u64,
    from: u32,
    to: u32,
) {
    timeline.push(TimelineEvent {
        service_id,
        kind,
        tick,
        from,
        to,
    });
    match kind {
        event_kind::JOB_PHASE => match to {
            job_state::RUNNING => counters.jobs_started = counters.jobs_started.saturating_add(1),
            job_state::SUCCEEDED => {
                counters.jobs_succeeded = counters.jobs_succeeded.saturating_add(1);
            }
            job_state::FAILED | job_state::UNSUPPORTED => {
                counters.jobs_failed = counters.jobs_failed.saturating_add(1);
            }
            _ => {}
        },
        event_kind::FRAME_PACING => {
            counters.long_frames = counters.long_frames.saturating_add(1);
        }
        event_kind::NET_SELFTEST => match to as u64 {
            net_selftest::PASSED => {
                counters.net_selftests_passed = counters.net_selftests_passed.saturating_add(1);
            }
            net_selftest::FAILED => {
                counters.net_selftests_failed = counters.net_selftests_failed.saturating_add(1);
            }
            _ => {}
        },
        event_kind::PRESSURE => {
            counters.pressure_level = to;
            counters.pressure_transitions = counters.pressure_transitions.saturating_add(1);
        }
        _ => {}
    }
}

/// Present-count sampler threshold: when a compositor feed sample shows the
/// present counter jumped by at least this many coalesced presentations
/// since the previous sample, the compositor fell behind its damage rate
/// between samples (healthy pacing coalesces around two ticks per present).
pub(crate) const PACING_JUMP_PRESENTS: u64 = 4;

/// Sampler state for classifying domain feed records into timeline events.
/// Developer build-job records map directly; graphics presents advance a
/// present-count sampler that flags pacing anomalies.
#[derive(Clone, Copy)]
pub(crate) struct DomainSampler {
    last_present_tick: Option<u64>,
    last_present_count: Option<u64>,
}

/// Classifies one raw record payload pair into the net-selftest timeline
/// kind, or `None` when the record is not a tagged selftest record. Shared by
/// the stream classifier and the retained-ring sweep so both paths stay in
/// lockstep.
pub(crate) fn classify_net_selftest_record(arg0: u64, arg1: u64) -> Option<(u32, u32, u32)> {
    if arg0 != net_selftest::ARG0_TAG {
        return None;
    }
    let to = match arg1 {
        net_selftest::PASSED => net_selftest::PASSED as u32,
        net_selftest::FAILED => net_selftest::FAILED as u32,
        // Begin records enter the ring but stay uncounted.
        other => other.min(u32::MAX as u64) as u32,
    };
    Some((event_kind::NET_SELFTEST, 0, to))
}

/// Maps a kernel pressure record (`arg0` = from-level, `arg1` = to-level
/// discriminants) onto the PRESSURE timeline kind, or `None` for unknown
/// level encodings so malformed records never enter the ring.
pub(crate) fn classify_pressure_record(arg0: u64, arg1: u64) -> Option<(u32, u32, u32)> {
    let from = level_from_word(arg0)?;
    let to = level_from_word(arg1)?;
    Some((event_kind::PRESSURE, from, to))
}

fn level_from_word(word: u64) -> Option<u32> {
    match word {
        0 => Some(pressure_level::NORMAL),
        1 => Some(pressure_level::TIGHT),
        2 => Some(pressure_level::CRITICAL),
        _ => None,
    }
}

impl DomainSampler {
    pub(crate) const fn new() -> Self {
        Self {
            last_present_tick: None,
            last_present_count: None,
        }
    }

    /// Classifies one log-stream record into at most one domain event kind
    /// with its `from`/`to` payload. Graphics presents always advance the
    /// sampler even when they do not raise an anomaly. Unsupported build
    /// targets arrive as `DeveloperBuildFailed` Warn records with `arg1 == 1`
    /// and are kept distinct from genuine failures for the final-state tally.
    /// Network selftest records are identified by the reserved `arg0` tag;
    /// ping records on the same event never match it.
    pub(crate) fn classify(
        &mut self,
        domain: LogDomain,
        event: LogEvent,
        severity: LogSeverity,
        arg0: u64,
        arg1: u64,
        tick: u64,
    ) -> Option<(u32, u32, u32)> {
        match (domain, event) {
            (LogDomain::Developer, LogEvent::DeveloperBuildStarted) => {
                Some((event_kind::JOB_PHASE, job_state::QUEUED, job_state::RUNNING))
            }
            (LogDomain::Developer, LogEvent::DeveloperBuildFinished) => Some((
                event_kind::JOB_PHASE,
                job_state::RUNNING,
                job_state::SUCCEEDED,
            )),
            (LogDomain::Developer, LogEvent::DeveloperBuildFailed) => {
                let to = if severity <= LogSeverity::Warn && arg1 == 1 {
                    job_state::UNSUPPORTED
                } else {
                    job_state::FAILED
                };
                Some((event_kind::JOB_PHASE, job_state::RUNNING, to))
            }
            (LogDomain::Network, LogEvent::NetworkProbeCompleted) => {
                classify_net_selftest_record(arg0, arg1)
            }
            (LogDomain::Kernel, LogEvent::KernelPressureChanged) => {
                classify_pressure_record(arg0, arg1)
            }
            (LogDomain::Graphics, LogEvent::CompositorPresented) => {
                let previous_tick = self.last_present_tick.replace(tick);
                let previous_count = self.last_present_count.replace(arg1);
                let delta_count = arg1.saturating_sub(previous_count.unwrap_or(0));
                let previous_tick = previous_tick?;
                if delta_count < PACING_JUMP_PRESENTS {
                    return None;
                }
                Some((
                    event_kind::FRAME_PACING,
                    delta_count.min(u32::MAX as u64) as u32,
                    tick.saturating_sub(previous_tick).min(u32::MAX as u64) as u32,
                ))
            }
            _ => None,
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

    /// Drilldown: fills `out` with retained events matching `filter`, in push
    /// order, after skipping the first `skip` matches so callers can page
    /// through the window in bounded replies.
    pub(crate) fn query_filtered(
        &self,
        filter: TimelineFilter,
        skip: usize,
        out: &mut [TimelineEvent],
    ) -> usize {
        let mut matched = 0usize;
        let mut written = 0usize;
        for event in self.iter_oldest_first() {
            if !filter.matches(&event) {
                continue;
            }
            if matched >= skip && written < out.len() {
                out[written] = event;
                written += 1;
            }
            matched += 1;
        }
        written
    }

    /// Total retained events matching `filter` — lets drilldown clients size
    /// pagination before walking pages.
    pub(crate) fn count_filtered(&self, filter: TimelineFilter) -> usize {
        self.iter_oldest_first()
            .filter(|event| filter.matches(event))
            .count()
    }

    /// Per-service stat line over the retained window: total events,
    /// first/last seen ticks, and per-kind counts capped at
    /// [`SERVICE_KINDS_CAP`] buckets.
    pub(crate) fn service_stats(&self, service_id: u32) -> ServiceStats {
        let mut stats = ServiceStats::empty(service_id);
        for event in self.iter_oldest_first() {
            if event.service_id != service_id {
                continue;
            }
            stats.total += 1;
            if stats.first_tick.is_none() {
                stats.first_tick = Some(event.tick);
            }
            stats.last_tick = Some(event.tick);

            let existing = stats.per_kind[..stats.per_kind_len]
                .iter_mut()
                .find(|(kind, _)| *kind == event.kind);
            match existing {
                Some((_, count)) => *count += 1,
                None if stats.per_kind_len < SERVICE_KINDS_CAP => {
                    stats.per_kind[stats.per_kind_len] = (event.kind, 1);
                    stats.per_kind_len += 1;
                }
                None => {}
            }
        }
        stats
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

/// Drilldown predicate: optional service-id and event-kind constraints
/// composed conjunctively (`None` = unconstrained).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TimelineFilter {
    pub service_id: Option<u32>,
    pub kind: Option<u32>,
}

impl TimelineFilter {
    #[allow(dead_code)]
    pub(crate) const fn any() -> Self {
        Self {
            service_id: None,
            kind: None,
        }
    }

    fn matches(&self, event: &TimelineEvent) -> bool {
        self.service_id.is_none_or(|id| event.service_id == id)
            && self.kind.is_none_or(|kind| event.kind == kind)
    }
}

/// Per-service stat line aggregated over the retained window: total events
/// for the service, first/last seen ticks, and counts by event kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ServiceStats {
    pub service_id: u32,
    pub total: usize,
    pub first_tick: Option<u64>,
    pub last_tick: Option<u64>,
    pub per_kind: [(u32, usize); SERVICE_KINDS_CAP],
    pub per_kind_len: usize,
}

impl ServiceStats {
    pub(crate) const fn empty(service_id: u32) -> Self {
        Self {
            service_id,
            total: 0,
            first_tick: None,
            last_tick: None,
            per_kind: [(0, 0); SERVICE_KINDS_CAP],
            per_kind_len: 0,
        }
    }

    /// Packs into the 16-word STATS_REPLY envelope:
    /// `[0]` service id | total<<32, `[1]` distinct kinds shown,
    /// `[2]` first tick (0 = none), `[3]` last tick (0 = none),
    /// `[4..16]` up to six `(kind | count<<32)` rows.
    pub(crate) fn pack_reply_words(&self) -> [u64; 16] {
        let mut words = [0u64; 16];
        words[0] = self.service_id as u64 | ((self.total.min(u32::MAX as usize) as u64) << 32);
        words[1] = self.per_kind_len as u64;
        words[2] = self.first_tick.unwrap_or(0);
        words[3] = self.last_tick.unwrap_or(0);
        for slot in 0..self.per_kind_len.min(6) {
            let (kind, count) = self.per_kind[slot];
            words[4 + slot] = kind as u64 | ((count.min(u32::MAX as usize) as u64) << 32);
        }
        words
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SID_A: u32 = 3;
    const SID_B: u32 = 5;
    const KERNEL_SID: u32 = 2;

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

    fn drilldown_fixture() -> Timeline {
        let mut timeline = Timeline::new();
        timeline.push(ev(SID_A, event_kind::RESTART, 10));
        timeline.push(ev(SID_B, event_kind::CRASH, 20));
        timeline.push(ev(SID_A, event_kind::CRASH, 30));
        timeline.push(ev(SID_A, event_kind::RESTART, 40));
        timeline.push(ev(SID_B, event_kind::RESTART, 50));
        timeline
    }

    #[test]
    fn drilldown_filter_composes_service_and_kind() {
        let timeline = drilldown_fixture();
        let mut out = [TimelineEvent::zeroed(); TIMELINE_CAP];

        // Service only.
        let count = timeline.query_filtered(
            TimelineFilter {
                service_id: Some(SID_A),
                kind: None,
            },
            0,
            &mut out,
        );
        assert_eq!(count, 3);
        assert_eq!((out[0].tick, out[1].tick, out[2].tick), (10, 30, 40));

        // Kind only.
        let count = timeline.query_filtered(
            TimelineFilter {
                service_id: None,
                kind: Some(event_kind::CRASH),
            },
            0,
            &mut out,
        );
        assert_eq!(count, 2);
        assert_eq!(out[0].service_id, SID_B);
        assert_eq!(out[1].service_id, SID_A);

        // Both constraints AND together.
        let count = timeline.query_filtered(
            TimelineFilter {
                service_id: Some(SID_B),
                kind: Some(event_kind::CRASH),
            },
            0,
            &mut out,
        );
        assert_eq!(count, 1);
        assert_eq!(out[0].tick, 20);

        // Unconstrained filter returns the whole window in push order.
        let count = timeline.query_filtered(TimelineFilter::any(), 0, &mut out);
        assert_eq!(count, 5);
        assert_eq!(out[0].tick, 10);
        assert_eq!(out[4].tick, 50);

        // Matching counts agree with the returned pages.
        for service_id in [SID_A, SID_B] {
            for kind in [None, Some(event_kind::RESTART), Some(event_kind::CRASH)] {
                let filter = TimelineFilter {
                    service_id: Some(service_id),
                    kind,
                };
                assert_eq!(timeline.count_filtered(filter), {
                    let mut seen = 0;
                    for slot in 0..5 {
                        if out[slot].service_id == service_id
                            && kind.is_none_or(|k| out[slot].kind == k)
                        {
                            seen += 1;
                        }
                    }
                    seen
                });
            }
        }
    }

    #[test]
    fn drilldown_pagination_bounds_skip_pages_and_truncate() {
        let timeline = drilldown_fixture();
        let mut page = [TimelineEvent::zeroed(); 2];

        // Page size bounds output even when more matches remain.
        let written = timeline.query_filtered(
            TimelineFilter {
                service_id: Some(SID_A),
                kind: None,
            },
            0,
            &mut page,
        );
        assert_eq!(written, 2);
        assert_eq!((page[0].tick, page[1].tick), (10, 30));

        // Skip walks forward through the matched stream.
        let written = timeline.query_filtered(
            TimelineFilter {
                service_id: Some(SID_A),
                kind: None,
            },
            2,
            &mut page,
        );
        assert_eq!(written, 1);
        assert_eq!(page[0].tick, 40);

        // Skipping past the end of the match set is empty, not a panic.
        for skip in [3usize, 99] {
            let written = timeline.query_filtered(
                TimelineFilter {
                    service_id: Some(SID_A),
                    kind: None,
                },
                skip,
                &mut page,
            );
            assert_eq!(written, 0);
        }

        // Zero-capacity buffer returns nothing but reports via count_filtered.
        let written = timeline.query_filtered(TimelineFilter::any(), 0, &mut []);
        assert_eq!(written, 0);
        assert_eq!(timeline.count_filtered(TimelineFilter::any()), 5);
    }

    #[test]
    fn service_stats_counts_kinds_first_last_within_window() {
        let mut timeline = Timeline::new();
        timeline.push(ev(SID_A, event_kind::SEED, 100));
        timeline.push(ev(SID_B, event_kind::SEED, 110));
        timeline.push(ev(SID_A, event_kind::CRASH, 120));
        timeline.push(ev(SID_A, event_kind::CRASH, 130));
        timeline.push(ev(SID_A, event_kind::RESTART, 140));

        let stats = timeline.service_stats(SID_A);
        assert_eq!(stats.service_id, SID_A);
        assert_eq!(stats.total, 4);
        assert_eq!(stats.first_tick, Some(100));
        assert_eq!(stats.last_tick, Some(140));
        assert_eq!(stats.per_kind_len, 3);
        assert!(stats.per_kind[..stats.per_kind_len].contains(&(event_kind::SEED, 1)));
        assert!(stats.per_kind[..stats.per_kind_len].contains(&(event_kind::CRASH, 2)));
        assert!(stats.per_kind[..stats.per_kind_len].contains(&(event_kind::RESTART, 1)));

        // Unknown service id yields an all-zero stat line.
        assert_eq!(timeline.service_stats(42), ServiceStats::empty(42));

        // Window bounds hold under ring eviction: first/last reflect the
        // retained window, not total pushes.
        for tick in 200..(200 + TIMELINE_CAP as u64) {
            timeline.push(ev(SID_A, event_kind::STATE_CHANGE, tick));
        }
        let stats = timeline.service_stats(SID_A);
        assert_eq!(timeline.len(), TIMELINE_CAP);
        assert_eq!(stats.total, TIMELINE_CAP);
        assert_eq!(stats.first_tick, Some(200));
        assert_eq!(stats.last_tick, Some(200 + TIMELINE_CAP as u64 - 1));
        assert_eq!(stats.per_kind_len, 1);
        assert_eq!(stats.per_kind[0], (event_kind::STATE_CHANGE, TIMELINE_CAP));
    }

    #[test]
    fn service_stats_reply_packs_stat_line_layout() {
        let mut timeline = Timeline::new();
        timeline.push(ev(SID_A, event_kind::CRASH, 120));
        timeline.push(ev(SID_A, event_kind::CRASH, 130));

        let stats = timeline.service_stats(SID_A);
        let words = stats.pack_reply_words();
        assert_eq!(words.len(), 16);
        assert_eq!(words[0], SID_A as u64 | (2 << 32));
        assert_eq!(words[1], 1);
        assert_eq!(words[2], 120);
        assert_eq!(words[3], 130);
        assert_eq!(words[4], event_kind::CRASH as u64 | (2 << 32));
        assert!(words[5..].iter().all(|word| *word == 0));
        assert_eq!(
            ServiceStats::empty(SID_B).pack_reply_words()[0],
            SID_B as u64
        );
    }

    const DEV_SID: u32 = 16;

    fn ingest(
        timeline: &mut Timeline,
        counters: &mut DomainCounters,
        kind: u32,
        to: u32,
        tick: u64,
    ) {
        ingest_domain_event(timeline, counters, DEV_SID, kind, tick, 0, to);
    }

    #[test]
    fn domain_job_transitions_tally_by_final_state() {
        let mut timeline = Timeline::new();
        let mut counters = DomainCounters::empty();
        ingest(
            &mut timeline,
            &mut counters,
            event_kind::JOB_PHASE,
            job_state::RUNNING,
            10,
        );
        ingest(
            &mut timeline,
            &mut counters,
            event_kind::JOB_PHASE,
            job_state::SUCCEEDED,
            20,
        );
        ingest(
            &mut timeline,
            &mut counters,
            event_kind::JOB_PHASE,
            job_state::RUNNING,
            30,
        );
        ingest(
            &mut timeline,
            &mut counters,
            event_kind::JOB_PHASE,
            job_state::FAILED,
            40,
        );
        ingest(
            &mut timeline,
            &mut counters,
            event_kind::JOB_PHASE,
            job_state::UNSUPPORTED,
            50,
        );

        assert_eq!(counters.jobs_started, 2);
        assert_eq!(counters.jobs_succeeded, 1);
        assert_eq!(counters.jobs_failed, 2);
        assert_eq!(counters.long_frames, 0);
        assert_eq!(timeline.len(), 5);

        let mut out = [TimelineEvent::zeroed(); TIMELINE_CAP];
        assert_eq!(timeline.query_since(0, &mut out), 5);
        assert_eq!(out[0].kind, event_kind::JOB_PHASE);
        assert_eq!(out[0].to, job_state::RUNNING);
        assert_eq!(out[4].tick, 50);
    }

    #[test]
    fn domain_frame_pacing_counts_and_unknown_kinds_stay_uncounted() {
        let mut timeline = Timeline::new();
        let mut counters = DomainCounters::empty();
        ingest(
            &mut timeline,
            &mut counters,
            event_kind::FRAME_PACING,
            12,
            100,
        );
        ingest(
            &mut timeline,
            &mut counters,
            event_kind::FRAME_PACING,
            6,
            200,
        );
        ingest(&mut timeline, &mut counters, 99, 1, 300);

        assert_eq!(counters.long_frames, 2);
        assert_eq!(counters.jobs_started, 0);
        assert_eq!(timeline.len(), 3);
        assert_eq!(
            counters,
            DomainCounters {
                jobs_started: 0,
                jobs_succeeded: 0,
                jobs_failed: 0,
                long_frames: 2,
                net_selftests_passed: 0,
                net_selftests_failed: 0,
                pressure_level: pressure_level::NORMAL,
                pressure_transitions: 0
            }
        );
    }

    #[test]
    fn domain_counters_survive_ring_window_eviction() {
        let mut timeline = Timeline::new();
        let mut counters = DomainCounters::empty();
        ingest(&mut timeline, &mut counters, event_kind::FRAME_PACING, 8, 1);
        for tick in 2..(TIMELINE_CAP as u64 + 4) {
            ingest(
                &mut timeline,
                &mut counters,
                event_kind::JOB_PHASE,
                job_state::SUCCEEDED,
                tick,
            );
        }

        assert!(timeline.len() < counters.jobs_succeeded as usize + 1);
        assert_eq!(timeline.len(), TIMELINE_CAP);
        assert!(counters.jobs_succeeded >= TIMELINE_CAP as u64);
        assert_eq!(counters.long_frames, 1);
        assert_eq!(
            counters.pack_reply_words()[3],
            1,
            "long_frames rides reply word 15"
        );
    }

    #[test]
    fn domain_counter_reply_pack_matches_layout() {
        let counters = DomainCounters {
            jobs_started: 3,
            jobs_succeeded: 2,
            jobs_failed: 1,
            long_frames: 7,
            net_selftests_passed: 11,
            net_selftests_failed: 13,
            pressure_level: pressure_level::TIGHT,
            pressure_transitions: 4,
        };
        assert_eq!(
            counters.pack_reply_words(),
            [3, 2, 1, 7, 11, 13, pressure_level::TIGHT as u64, 4]
        );
        assert_eq!(
            DomainCounters::empty().pack_reply_words(),
            [0, 0, 0, 0, 0, 0, pressure_level::NORMAL as u64, 0]
        );
    }

    #[test]
    fn net_selftest_records_classify_by_tag_and_phase() {
        let mut sampler = DomainSampler::new();
        // Tagged begin record enters the ring (kind NET_SELFTEST, to = BEGIN).
        assert_eq!(
            classify(
                &mut sampler,
                LogDomain::Network,
                LogEvent::NetworkProbeCompleted,
                LogSeverity::Debug,
                net_selftest::ARG0_TAG,
                net_selftest::BEGIN,
                10
            ),
            Some((event_kind::NET_SELFTEST, 0, net_selftest::BEGIN as u32))
        );
        // Pass/fail records map to their final states regardless of severity.
        for (severity, phase) in [
            (LogSeverity::Info, net_selftest::PASSED),
            (LogSeverity::Error, net_selftest::FAILED),
        ] {
            assert_eq!(
                classify(
                    &mut sampler,
                    LogDomain::Network,
                    LogEvent::NetworkProbeCompleted,
                    severity,
                    net_selftest::ARG0_TAG,
                    phase,
                    20
                ),
                Some((event_kind::NET_SELFTEST, 0, phase as u32))
            );
        }
        // Ping records on the same event never carry the tag -> unclassified.
        assert_eq!(
            classify(
                &mut sampler,
                LogDomain::Network,
                LogEvent::NetworkProbeCompleted,
                LogSeverity::Info,
                0x7F00_0001,
                1,
                30
            ),
            None
        );
        // Wrong event with the tag present stays unclassified.
        assert_eq!(
            classify(
                &mut sampler,
                LogDomain::Network,
                LogEvent::NetworkLeaseChanged,
                LogSeverity::Info,
                net_selftest::ARG0_TAG,
                net_selftest::PASSED,
                40
            ),
            None
        );
    }

    #[test]
    fn net_selftest_ring_sweep_path_matches_stream_classification() {
        let mut sampler = DomainSampler::new();
        let mut timeline = Timeline::new();
        let mut counters = DomainCounters::empty();
        // Tagged records decode identically on both delivery paths.
        for (tick, arg1) in [(5u64, net_selftest::BEGIN), (9, net_selftest::PASSED)] {
            let classified = classify_net_selftest_record(net_selftest::ARG0_TAG, arg1).and_then(
                |(kind, from, to)| {
                    sampler
                        .classify(
                            LogDomain::Network,
                            LogEvent::NetworkProbeCompleted,
                            LogSeverity::Debug,
                            net_selftest::ARG0_TAG,
                            arg1,
                            tick,
                        )
                        .map(|stream| assert_eq!(stream, (kind, from, to)))
                },
            );
            assert!(classified.is_some());
            let (kind, from, to) =
                classify_net_selftest_record(net_selftest::ARG0_TAG, arg1).unwrap();
            ingest_domain_event(&mut timeline, &mut counters, 12, kind, tick, from, to);
        }
        // Untagged ping payloads stay unclassified on both paths.
        assert!(classify_net_selftest_record(0x7F00_0001, 1).is_none());
        assert_eq!(
            sampler.classify(
                LogDomain::Network,
                LogEvent::NetworkProbeCompleted,
                LogSeverity::Info,
                0x7F00_0001,
                1,
                30
            ),
            None
        );
        assert_eq!(timeline.len(), 2);
        assert_eq!(counters.net_selftests_passed, 1);
        assert_eq!(counters.net_selftests_failed, 0);
    }

    #[test]
    fn net_selftest_counters_tally_passed_and_failed() {
        let mut timeline = Timeline::new();
        let mut counters = DomainCounters::empty();
        ingest_domain_event(
            &mut timeline,
            &mut counters,
            12,
            event_kind::NET_SELFTEST,
            5,
            0,
            net_selftest::BEGIN as u32,
        );
        ingest_domain_event(
            &mut timeline,
            &mut counters,
            12,
            event_kind::NET_SELFTEST,
            9,
            0,
            net_selftest::PASSED as u32,
        );
        ingest_domain_event(
            &mut timeline,
            &mut counters,
            12,
            event_kind::NET_SELFTEST,
            15,
            0,
            net_selftest::FAILED as u32,
        );
        ingest_domain_event(
            &mut timeline,
            &mut counters,
            12,
            event_kind::NET_SELFTEST,
            21,
            0,
            net_selftest::PASSED as u32,
        );

        assert_eq!(counters.net_selftests_passed, 2);
        assert_eq!(counters.net_selftests_failed, 1);
        assert_eq!(timeline.len(), 4);
        let mut out = [TimelineEvent::zeroed(); TIMELINE_CAP];
        assert_eq!(timeline.query_since(0, &mut out), 4);
        assert_eq!(out[0].kind, event_kind::NET_SELFTEST);
        assert_eq!(out[0].to, net_selftest::BEGIN as u32);
        assert_eq!(
            counters.pack_reply_words()[4],
            2,
            "passed rides reply word 16"
        );
        assert_eq!(
            counters.pack_reply_words()[5],
            1,
            "failed rides reply word 17"
        );
    }

    #[test]
    fn pressure_records_classify_to_level_pairs_and_reject_unknowns() {
        let mut sampler = DomainSampler::new();
        // Normal -> Tight -> Critical -> Normal round trip.
        assert_eq!(
            classify(
                &mut sampler,
                LogDomain::Kernel,
                LogEvent::KernelPressureChanged,
                LogSeverity::Warn,
                pressure_level::NORMAL as u64,
                pressure_level::TIGHT as u64,
                10
            ),
            Some((
                event_kind::PRESSURE,
                pressure_level::NORMAL,
                pressure_level::TIGHT
            ))
        );
        assert_eq!(
            classify(
                &mut sampler,
                LogDomain::Kernel,
                LogEvent::KernelPressureChanged,
                LogSeverity::Error,
                pressure_level::TIGHT as u64,
                pressure_level::CRITICAL as u64,
                20
            ),
            Some((
                event_kind::PRESSURE,
                pressure_level::TIGHT,
                pressure_level::CRITICAL
            ))
        );
        assert_eq!(
            classify(
                &mut sampler,
                LogDomain::Kernel,
                LogEvent::KernelPressureChanged,
                LogSeverity::Info,
                pressure_level::CRITICAL as u64,
                pressure_level::NORMAL as u64,
                30
            ),
            Some((
                event_kind::PRESSURE,
                pressure_level::CRITICAL,
                pressure_level::NORMAL
            ))
        );
        // Unknown level encodings never enter the ring.
        assert_eq!(
            classify(
                &mut sampler,
                LogDomain::Kernel,
                LogEvent::KernelPressureChanged,
                LogSeverity::Error,
                0,
                3,
                40
            ),
            None
        );
        assert_eq!(
            classify(
                &mut sampler,
                LogDomain::Kernel,
                LogEvent::KernelPressureChanged,
                LogSeverity::Error,
                9,
                0,
                50
            ),
            None
        );
        // KernelTrap records on the same domain stay unclassified here.
        assert_eq!(
            classify(
                &mut sampler,
                LogDomain::Kernel,
                LogEvent::KernelTrap,
                LogSeverity::Error,
                0,
                1,
                60
            ),
            None
        );
    }

    #[test]
    fn pressure_ingest_updates_rollup_state_and_reply_words() {
        let mut timeline = Timeline::new();
        let mut counters = DomainCounters::empty();
        assert_eq!(counters.pressure_level, pressure_level::NORMAL);

        ingest_domain_event(
            &mut timeline,
            &mut counters,
            KERNEL_SID,
            event_kind::PRESSURE,
            10,
            pressure_level::NORMAL,
            pressure_level::TIGHT,
        );
        ingest_domain_event(
            &mut timeline,
            &mut counters,
            KERNEL_SID,
            event_kind::PRESSURE,
            20,
            pressure_level::TIGHT,
            pressure_level::CRITICAL,
        );

        assert_eq!(counters.pressure_level, pressure_level::CRITICAL);
        assert_eq!(counters.pressure_transitions, 2);
        let mut out = [TimelineEvent::zeroed(); TIMELINE_CAP];
        assert_eq!(timeline.query_since(0, &mut out), 2);
        assert_eq!(out[0].kind, event_kind::PRESSURE);
        assert_eq!(out[0].from, pressure_level::NORMAL);
        assert_eq!(out[0].to, pressure_level::TIGHT);
        assert_eq!(out[1].tick, 20);
        let packed = counters.pack_reply_words();
        assert_eq!(
            packed[6],
            pressure_level::CRITICAL as u64,
            "level rides reply word 18"
        );
        assert_eq!(packed[7], 2, "transitions ride reply word 19");
    }

    fn classify(
        sampler: &mut DomainSampler,
        domain: LogDomain,
        event: LogEvent,
        severity: LogSeverity,
        arg0: u64,
        arg1: u64,
        tick: u64,
    ) -> Option<(u32, u32, u32)> {
        sampler.classify(domain, event, severity, arg0, arg1, tick)
    }

    #[test]
    fn build_records_classify_to_job_phase_transitions() {
        let mut sampler = DomainSampler::new();
        assert_eq!(
            classify(
                &mut sampler,
                LogDomain::Developer,
                LogEvent::DeveloperBuildStarted,
                LogSeverity::Info,
                0,
                7,
                10
            ),
            Some((event_kind::JOB_PHASE, job_state::QUEUED, job_state::RUNNING))
        );
        assert_eq!(
            classify(
                &mut sampler,
                LogDomain::Developer,
                LogEvent::DeveloperBuildFinished,
                LogSeverity::Info,
                0,
                4096,
                20
            ),
            Some((
                event_kind::JOB_PHASE,
                job_state::RUNNING,
                job_state::SUCCEEDED
            ))
        );
        // Genuine failure encodings (route/sandbox/exit-code payloads) stay Failed.
        for (severity, arg1) in [
            (LogSeverity::Error, 2),
            (LogSeverity::Warn, 0x100),
            (LogSeverity::Error, 139),
        ] {
            assert_eq!(
                classify(
                    &mut sampler,
                    LogDomain::Developer,
                    LogEvent::DeveloperBuildFailed,
                    severity,
                    0,
                    arg1,
                    30
                ),
                Some((event_kind::JOB_PHASE, job_state::RUNNING, job_state::FAILED))
            );
        }
        // Warn + arg1 == 1 is the unsupported-target final state.
        assert_eq!(
            classify(
                &mut sampler,
                LogDomain::Developer,
                LogEvent::DeveloperBuildFailed,
                LogSeverity::Warn,
                0,
                1,
                40
            ),
            Some((
                event_kind::JOB_PHASE,
                job_state::RUNNING,
                job_state::UNSUPPORTED
            ))
        );
    }

    #[test]
    fn unrelated_stream_records_classify_to_none() {
        let mut sampler = DomainSampler::new();
        assert_eq!(
            classify(
                &mut sampler,
                LogDomain::Developer,
                LogEvent::DeveloperCatalogLoaded,
                LogSeverity::Info,
                0,
                0,
                5
            ),
            None
        );
        assert_eq!(
            classify(
                &mut sampler,
                LogDomain::Network,
                LogEvent::NetworkInterfaceReady,
                LogSeverity::Info,
                0,
                0,
                6
            ),
            None
        );
    }

    #[test]
    fn present_sampler_first_sample_is_baseline_only_then_flags_jumps() {
        let mut sampler = DomainSampler::new();
        assert_eq!(
            classify(
                &mut sampler,
                LogDomain::Graphics,
                LogEvent::CompositorPresented,
                LogSeverity::Info,
                0,
                2,
                100
            ),
            None
        );
        assert_eq!(
            classify(
                &mut sampler,
                LogDomain::Graphics,
                LogEvent::CompositorPresented,
                LogSeverity::Info,
                0,
                8,
                130
            ),
            Some((event_kind::FRAME_PACING, 6, 30))
        );
    }

    #[test]
    fn present_sampler_small_delta_and_counter_regress_stay_quiet() {
        let mut sampler = DomainSampler::new();
        assert_eq!(
            classify(
                &mut sampler,
                LogDomain::Graphics,
                LogEvent::CompositorPresented,
                LogSeverity::Info,
                0,
                4,
                10
            ),
            None
        );
        assert_eq!(
            classify(
                &mut sampler,
                LogDomain::Graphics,
                LogEvent::CompositorPresented,
                LogSeverity::Info,
                0,
                5,
                20
            ),
            None
        );
        assert_eq!(
            classify(
                &mut sampler,
                LogDomain::Graphics,
                LogEvent::CompositorPresented,
                LogSeverity::Info,
                0,
                1,
                30
            ),
            None
        );
    }
}
