//! Per-run phase accounting for developer jobs. Each job records
//! boot-local monotonic ticks at its lifecycle transitions
//! (queue -> start -> build-tool-exit -> artifact-save -> finish) so the
//! shell `dev profile` surface can render a phase table without a shared
//! ABI edit. Ticks come from the kernel monotonic clock; the tick rate is
//! 100 Hz on every supported platform (arch/*/TIMER_TICK_HZ), carried on
//! the wire so readers never guess.

use serviceos_userspace_runtime as rt;

/// Monotonic tick rate of the kernel time manager. Kept here (not derived)
/// because the ABI exposes no rate query; every platform image passes 100
/// (arch/x86_64/src/interrupts.rs, aarch64/riscv64 images).
pub(crate) const TICK_HZ: u64 = 100;

/// Phase slots in lifecycle order. A zero value means "not reached" — the
/// kernel monotonic clock never reports 0 as a stamp in practice, and the
/// valid mask carries the authoritative record.
pub(crate) const PHASE_QUEUE: usize = 0;
pub(crate) const PHASE_START: usize = 1;
pub(crate) const PHASE_TOOL_EXIT: usize = 2;
pub(crate) const PHASE_ARTIFACT: usize = 3;
pub(crate) const PHASE_FINISH: usize = 4;
pub(crate) const PHASE_COUNT: usize = 5;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct JobTiming {
    pub(crate) ticks: [u64; PHASE_COUNT],
}

impl JobTiming {
    pub(crate) const fn empty() -> Self {
        Self {
            ticks: [0; PHASE_COUNT],
        }
    }

    /// First-write-wins per phase: a recorded stamp is never overwritten,
    /// so replays (report polled twice) cannot rewind the clock.
    pub(crate) fn record(&mut self, phase: usize, now: u64) {
        if phase < PHASE_COUNT && self.ticks[phase] == 0 {
            self.ticks[phase] = now;
        }
    }

    pub(crate) fn record_queue(&mut self, now: u64) {
        self.record(PHASE_QUEUE, now);
    }

    pub(crate) fn record_start(&mut self, now: u64) {
        self.record(PHASE_START, now);
    }

    pub(crate) fn record_tool_exit(&mut self, now: u64) {
        self.record(PHASE_TOOL_EXIT, now);
    }

    pub(crate) fn record_artifact(&mut self, now: u64) {
        self.record(PHASE_ARTIFACT, now);
    }

    pub(crate) fn record_finish(&mut self, now: u64) {
        self.record(PHASE_FINISH, now);
    }

    /// Bit per recorded phase, PHASE_QUEUE = bit 0 upward.
    pub(crate) fn valid_mask(&self) -> u32 {
        let mut mask = 0u32;
        for (bit, tick) in self.ticks.iter().enumerate() {
            if *tick != 0 {
                mask |= 1 << bit;
            }
        }
        mask
    }

    /// Queue-to-finish span in ticks; 0 while the run is unfinished.
    pub(crate) fn duration_ticks(&self) -> u64 {
        match (self.ticks[PHASE_QUEUE], self.ticks[PHASE_FINISH]) {
            (queue, finish) if queue != 0 && finish != 0 => finish.saturating_sub(queue),
            _ => 0,
        }
    }

    /// Delta between two recorded phases; 0 when either stamp is unset.
    /// Service-side reads go through the wire shapes; shell-side rendering
    /// mirrors this in its own decoder, so this stays test-only.
    #[cfg(test)]
    pub(crate) fn delta(from: usize, to: usize, ticks: &[u64; PHASE_COUNT]) -> u64 {
        if from >= PHASE_COUNT || to >= PHASE_COUNT {
            return 0;
        }
        match (ticks[from], ticks[to]) {
            (from_tick, to_tick) if from_tick != 0 && to_tick != 0 => {
                to_tick.saturating_sub(from_tick)
            }
            _ => 0,
        }
    }
}

/// Boot-local tick for job accounting. On the target this is the kernel
/// monotonic clock; host unit tests cannot issue kernel syscalls, so they
/// inject ticks explicitly and observe 0 here (same split as
/// runtime-service's now_tick).
pub(crate) fn now_tick() -> u64 {
    #[cfg(test)]
    {
        0
    }
    #[cfg(not(test))]
    {
        rt::monotonic_now().unwrap_or(0)
    }
}

/// Wire shape shared by the profile reply and the job-list tail: five
/// phase ticks plus a rate word packing the tick rate (low 32 bits) and
/// the recorded-phase valid mask (bits 32..37, PHASE_QUEUE = bit 32).
pub(crate) fn pack_timing_words(timing: &JobTiming) -> [u64; PHASE_COUNT + 1] {
    let mut words = [0u64; PHASE_COUNT + 1];
    words[..PHASE_COUNT].copy_from_slice(&timing.ticks);
    words[PHASE_COUNT] = u64::from(TICK_HZ) | ((u64::from(timing.valid_mask())) << 32);
    words
}

/// Decode the rate word: (tick_hz, valid_mask). Test-only here; the shell
/// decoder mirrors the same packing when it renders deltas in ms.
#[cfg(test)]
pub(crate) fn unpack_rate_word(word: u64) -> (u32, u32) {
    (word as u32, (word >> 32) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_first_write_only() {
        let mut timing = JobTiming::empty();
        timing.record_queue(10);
        timing.record_queue(50);
        assert_eq!(timing.ticks[PHASE_QUEUE], 10);
        timing.record_start(20);
        assert_eq!(timing.ticks[PHASE_START], 20);
    }

    #[test]
    fn deltas_monotonic_via_saturating_sub() {
        let mut timing = JobTiming::empty();
        timing.record_queue(100);
        timing.record_start(120);
        timing.record_tool_exit(180);
        timing.record_artifact(180);
        timing.record_finish(200);
        assert_eq!(
            JobTiming::delta(PHASE_QUEUE, PHASE_START, &timing.ticks),
            20
        );
        assert_eq!(
            JobTiming::delta(PHASE_START, PHASE_TOOL_EXIT, &timing.ticks),
            60
        );
        assert_eq!(
            JobTiming::delta(PHASE_TOOL_EXIT, PHASE_ARTIFACT, &timing.ticks),
            0
        );
        assert_eq!(
            JobTiming::delta(PHASE_ARTIFACT, PHASE_FINISH, &timing.ticks),
            20
        );
        assert_eq!(timing.duration_ticks(), 100);
    }

    #[test]
    fn unset_phases_report_zero_honestly() {
        let mut timing = JobTiming::empty();
        timing.record_queue(10);
        timing.record_finish(30);
        assert_eq!(
            JobTiming::delta(PHASE_START, PHASE_FINISH, &timing.ticks),
            0
        );
        assert_eq!(timing.duration_ticks(), 20);
        assert_eq!(
            timing.valid_mask(),
            (1 << PHASE_QUEUE) | (1 << PHASE_FINISH)
        );
    }

    #[test]
    fn out_of_bounds_delta_is_zero() {
        let mut timing = JobTiming::empty();
        timing.record_queue(10);
        assert_eq!(JobTiming::delta(0, 99, &timing.ticks), 0);
    }

    #[test]
    fn pack_words_carries_rate_and_mask() {
        let mut timing = JobTiming::empty();
        timing.record_queue(5);
        timing.record_start(7);
        let words = pack_timing_words(&timing);
        assert_eq!(words.len(), PHASE_COUNT + 1);
        assert_eq!(words[PHASE_QUEUE], 5);
        assert_eq!(words[PHASE_START], 7);
        assert_eq!(words[PHASE_TOOL_EXIT], 0);
        let (rate, mask) = unpack_rate_word(words[PHASE_COUNT]);
        assert_eq!(u64::from(rate), TICK_HZ);
        assert_eq!(mask, (1 << PHASE_QUEUE) | (1 << PHASE_START));
    }
}
