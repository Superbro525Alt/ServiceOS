//! Continuous-ping RTT statistics.
//!
//! Samples are milliseconds observed per echo reply; the summary is the
//! aggregate reported by DIAG_PING_STATS. Pure math, host-unit-testable.

use crate::consts::MAX_DIAG_PINGS;

#[derive(Clone, Copy)]
pub(crate) struct RttSamples {
    samples: [u64; MAX_DIAG_PINGS],
    count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RttSummary {
    pub(crate) received: usize,
    pub(crate) min_ms: u64,
    pub(crate) max_ms: u64,
    /// Integer mean of received samples.
    pub(crate) avg_ms: u64,
    /// Mean absolute deviation from `avg_ms` (integer jitter estimate).
    pub(crate) jitter_ms: u64,
}

impl RttSamples {
    pub(crate) const fn new() -> Self {
        Self {
            samples: [0; MAX_DIAG_PINGS],
            count: 0,
        }
    }

    pub(crate) fn push(&mut self, elapsed_ms: u64) {
        if self.count == self.samples.len() {
            return;
        }
        self.samples[self.count] = elapsed_ms;
        self.count += 1;
    }

    /// Aggregate over received samples. None when every probe timed out.
    pub(crate) fn summarize(&self) -> Option<RttSummary> {
        if self.count == 0 {
            return None;
        }
        let mut total = 0u64;
        let mut min = u64::MAX;
        let mut max = 0u64;
        for index in 0..self.count {
            let sample = self.samples[index];
            total = total.saturating_add(sample);
            min = min.min(sample);
            max = max.max(sample);
        }
        let avg = total / self.count as u64;
        let mut deviation_total = 0u64;
        for index in 0..self.count {
            let sample = self.samples[index];
            deviation_total += sample.abs_diff(avg);
        }
        Some(RttSummary {
            received: self.count,
            min_ms: min,
            max_ms: max,
            avg_ms: avg,
            jitter_ms: deviation_total / self.count as u64,
        })
    }
}

/// Loss in tenths of a percent (permil of sent probes that timed out).
pub(crate) fn loss_permil(sent: usize, received: usize) -> u64 {
    if sent == 0 {
        return 0;
    }
    ((sent - received.min(sent)) as u64 * 1000) / sent as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_math_min_max_avg_jitter() {
        let mut samples = RttSamples::new();
        for elapsed in [12u64, 4, 20, 8] {
            samples.push(elapsed);
        }
        let summary = samples.summarize().expect("samples exist");
        assert_eq!(summary.received, 4);
        assert_eq!(summary.min_ms, 4);
        assert_eq!(summary.max_ms, 20);
        assert_eq!(summary.avg_ms, 11); // (12+4+20+8)/4
        // Deviations from 11: 1+7+9+3=20 → jitter 5.
        assert_eq!(summary.jitter_ms, 5);
    }

    #[test]
    fn empty_samples_have_no_summary() {
        assert!(RttSamples::new().summarize().is_none());
    }

    #[test]
    fn push_is_bounded_and_loss_math_holds() {
        let mut samples = RttSamples::new();
        for index in 0..MAX_DIAG_PINGS + 4 {
            samples.push(index as u64);
        }
        assert_eq!(
            samples.summarize().map(|summary| summary.received),
            Some(MAX_DIAG_PINGS)
        );

        assert_eq!(loss_permil(10, 10), 0);
        assert_eq!(loss_permil(10, 7), 300);
        assert_eq!(loss_permil(3, 0), 1000);
        assert_eq!(loss_permil(0, 0), 0);
        // Received above sent cannot produce negative loss.
        assert_eq!(loss_permil(2, 9), 0);
    }
}
