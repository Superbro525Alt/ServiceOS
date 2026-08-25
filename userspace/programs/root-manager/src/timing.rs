use core::fmt::Write;
use core::str;

use serviceos_abi::ServiceId;

use crate::state::MAX_SERVICE_SLOTS;
use crate::util::{fallback_logf, service_name};

/// Kernel tick rate is 100 Hz (kernel/core/src/task/mod.rs), so 1 tick = 10ms.
pub(crate) const TICK_MS: u64 = 10;

pub(crate) fn elapsed_ms(start_tick: u64, end_tick: u64) -> u64 {
    end_tick.saturating_sub(start_tick).saturating_mul(TICK_MS)
}

#[derive(Clone, Copy)]
pub(crate) struct TimingRecord {
    pub(crate) service_id: ServiceId,
    pub(crate) start_tick: u64,
    pub(crate) end_tick: u64,
}

#[derive(Clone, Copy)]
pub(crate) struct BringUpTiming {
    pub(crate) records: [TimingRecord; MAX_SERVICE_SLOTS],
    pub(crate) len: usize,
    pub(crate) graph_start_tick: u64,
}

impl BringUpTiming {
    pub(crate) const fn empty() -> Self {
        Self {
            records: [TimingRecord {
                service_id: ServiceId::RootManager,
                start_tick: 0,
                end_tick: 0,
            }; MAX_SERVICE_SLOTS],
            len: 0,
            graph_start_tick: 0,
        }
    }

    pub(crate) fn begin(&mut self, service_id: ServiceId, now: u64) {
        if self.len >= self.records.len() {
            return;
        }
        self.records[self.len] = TimingRecord {
            service_id,
            start_tick: now,
            end_tick: 0,
        };
        self.len += 1;
    }

    pub(crate) fn end(&mut self, service_id: ServiceId, now: u64) {
        if let Some(record) = self.records[..self.len]
            .iter_mut()
            .rev()
            .find(|record| record.service_id == service_id && record.end_tick == 0)
        {
            record.end_tick = now.max(record.start_tick);
        }
    }

    fn duration_ms(index: usize, records: &[TimingRecord; MAX_SERVICE_SLOTS]) -> Option<u64> {
        let record = records[index];
        if record.end_tick == 0 {
            None
        } else {
            Some(elapsed_ms(record.start_tick, record.end_tick))
        }
    }

    /// Indices of the three slowest completed records, slowest first.
    /// Ties break toward the earlier-started (earlier-indexed) record.
    pub(crate) fn slowest_three(&self) -> [usize; 3] {
        let mut picked = [false; MAX_SERVICE_SLOTS];
        let mut top = [usize::MAX; 3];
        for out in top.iter_mut() {
            let mut best: Option<(usize, u64)> = None;
            for index in 0..self.len {
                if picked[index] {
                    continue;
                }
                if let Some(duration) = Self::duration_ms(index, &self.records) {
                    if best.map(|(_, best_ms)| duration > best_ms).unwrap_or(true) {
                        best = Some((index, duration));
                    }
                }
            }
            match best {
                Some((index, _)) => {
                    picked[index] = true;
                    *out = index;
                }
                None => break,
            }
        }
        top
    }

    fn total_ms(&self) -> u64 {
        self.total_ticks().saturating_mul(TICK_MS)
    }

    fn total_ticks(&self) -> u64 {
        let mut latest = self.graph_start_tick;
        for record in self.records[..self.len].iter() {
            latest = latest.max(record.end_tick);
        }
        latest.saturating_sub(self.graph_start_tick)
    }
}

struct LogBuffer<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> LogBuffer<N> {
    fn new() -> Self {
        Self {
            bytes: [0u8; N],
            len: 0,
        }
    }

    fn as_str(&self) -> &str {
        match str::from_utf8(&self.bytes[..self.len]) {
            Ok(text) => text,
            Err(_) => "",
        }
    }
}

impl<const N: usize> Write for LogBuffer<N> {
    fn write_str(&mut self, fragment: &str) -> core::fmt::Result {
        let remaining = self.bytes.len() - self.len;
        let amount = fragment.len().min(remaining);
        self.bytes[self.len..self.len + amount].copy_from_slice(fragment[..amount].as_bytes());
        self.len += amount;
        Ok(())
    }
}

pub(crate) fn emit_timing_summary(timing: &BringUpTiming) {
    let mut slowest = LogBuffer::<160>::new();
    for index in timing.slowest_three() {
        if index == usize::MAX || index >= timing.len {
            continue;
        }
        let record = timing.records[index];
        let _ = write!(
            slowest,
            " {}={}ms",
            service_name(record.service_id),
            elapsed_ms(record.start_tick, record.end_tick)
        );
    }
    let _ = fallback_logf(format_args!(
        "startup timing: total={}ms/{}t slowest:{}",
        timing.total_ms(),
        timing.total_ticks(),
        slowest.as_str()
    ));

    let mut per_service = LogBuffer::<512>::new();
    for record in timing.records[..timing.len].iter() {
        let ms = if record.end_tick == 0 {
            u64::MAX
        } else {
            elapsed_ms(record.start_tick, record.end_tick)
        };
        let ticks = if record.end_tick == 0 {
            u64::MAX
        } else {
            record.end_tick.saturating_sub(record.start_tick)
        };
        let _ = write!(
            per_service,
            " {}={}/{}t",
            service_name(record.service_id),
            ms,
            ticks,
        );
    }
    let _ = fallback_logf(format_args!("startup order:{}", per_service.as_str()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_converts_ticks_to_ms() {
        assert_eq!(elapsed_ms(0, 10), 100);
        assert_eq!(elapsed_ms(5, 7), 20);
        assert_eq!(elapsed_ms(9, 4), 0);
    }

    #[test]
    fn begin_end_records_duration() {
        let mut timing = BringUpTiming::empty();
        timing.graph_start_tick = 100;
        timing.begin(ServiceId::Storage, 110);
        timing.end(ServiceId::Storage, 113);
        assert_eq!(timing.len, 1);
        assert_eq!(
            elapsed_ms(timing.records[0].start_tick, timing.records[0].end_tick),
            30
        );
    }

    #[test]
    fn slowest_three_orders_descending() {
        let mut timing = BringUpTiming::empty();
        timing.begin(ServiceId::Storage, 0);
        timing.end(ServiceId::Storage, 2);
        timing.begin(ServiceId::DesktopShell, 10);
        timing.end(ServiceId::DesktopShell, 60);
        timing.begin(ServiceId::Config, 70);
        timing.end(ServiceId::Config, 73);
        let top = timing.slowest_three();
        assert_eq!(top[0], 1);
        assert_eq!(top[1], 2);
        assert_eq!(top[2], 0);
    }

    #[test]
    fn slowest_three_breaks_ties_toward_earlier_record() {
        let mut timing = BringUpTiming::empty();
        timing.begin(ServiceId::Log, 0);
        timing.end(ServiceId::Log, 1);
        timing.begin(ServiceId::Config, 0);
        timing.end(ServiceId::Config, 1);
        let top = timing.slowest_three();
        assert_eq!(top[0], 0);
        assert_eq!(top[1], 1);
    }

    #[test]
    fn unfinished_records_are_excluded_from_slowest() {
        let mut timing = BringUpTiming::empty();
        timing.begin(ServiceId::Storage, 0);
        timing.end(ServiceId::Storage, 1);
        timing.begin(ServiceId::Graphics, 5);
        let top = timing.slowest_three();
        assert_eq!(top[0], 0);
        assert_eq!(top[1], usize::MAX);
    }
}
