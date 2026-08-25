pub(crate) const TIMELINE_CAP: usize = 64;
pub(crate) const TIMELINE_SERVICES_CAP: usize = 24;
pub(crate) const TIMELINE_REPLY_EVENTS: usize = 5;

use serviceos_userspace_runtime::{LogDomain, LogEvent, LogSeverity};

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
    pub const JOB_PHASE: u32 = 5;
    pub const FRAME_PACING: u32 = 6;
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
}

impl DomainCounters {
    pub(crate) const fn empty() -> Self {
        Self {
            jobs_started: 0,
            jobs_succeeded: 0,
            jobs_failed: 0,
            long_frames: 0,
        }
    }

    /// Packs into the extended SUMMARY_REPLY layout words `[12..16]`
    /// (legacy summary occupies `[0..12]`).
    pub(crate) fn pack_reply_words(&self) -> [u64; 4] {
        [
            self.jobs_started,
            self.jobs_succeeded,
            self.jobs_failed,
            self.long_frames,
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
    pub(crate) fn classify(
        &mut self,
        domain: LogDomain,
        event: LogEvent,
        severity: LogSeverity,
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
                long_frames: 2
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
        };
        assert_eq!(counters.pack_reply_words(), [3, 2, 1, 7]);
        assert_eq!(DomainCounters::empty().pack_reply_words(), [0, 0, 0, 0]);
    }

    fn classify(
        sampler: &mut DomainSampler,
        domain: LogDomain,
        event: LogEvent,
        severity: LogSeverity,
        arg1: u64,
        tick: u64,
    ) -> Option<(u32, u32, u32)> {
        sampler.classify(domain, event, severity, arg1, tick)
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
                1,
                30
            ),
            None
        );
    }
}
