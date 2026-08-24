use serviceos_abi::{LogEvent, LogSeverity, RawMessage};

pub(crate) const CRASH_QUERY_REQUEST_TAG: u32 = 0x108;
pub(crate) const CRASH_QUERY_REPLY_TAG: u32 = 0x109;

pub(crate) const CRASH_QUERY_OK: u64 = 0;
pub(crate) const CRASH_QUERY_NOT_FOUND: u64 = 1;
pub(crate) const CRASH_QUERY_REPLY_WORDS: usize = 12;

pub(crate) const MAX_CRASH_RECORDS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CrashRecord {
    pub log_sequence: u64,
    pub tick: u64,
    pub source: u32,
    pub severity: u32,
    pub domain: u32,
    pub event: u32,
    pub arg0: u64,
    pub arg1: u64,
    pub arg2: u64,
}

impl CrashRecord {
    pub(crate) const fn is_crash(severity: u32, event: u32) -> bool {
        severity >= LogSeverity::Error as u32
            || event == LogEvent::KernelTrap as u32
            || event == LogEvent::ServiceFailed as u32
    }

    pub(crate) fn fill_reply(&self, words: &mut [u64; 16]) {
        words[3] = self.log_sequence;
        words[4] = self.tick;
        words[5] = self.source as u64;
        words[6] = self.severity as u64;
        words[7] = self.domain as u64;
        words[8] = self.event as u64;
        words[9] = self.arg0;
        words[10] = self.arg1;
        words[11] = self.arg2;
    }
}

#[derive(Clone, Copy)]
pub(crate) struct CrashLog {
    records: [CrashRecord; MAX_CRASH_RECORDS],
    next_slot: usize,
    count: usize,
    total_seen: u64,
}

impl CrashLog {
    pub(crate) const fn new() -> Self {
        Self {
            records: [CrashRecord {
                log_sequence: 0,
                tick: 0,
                source: 0,
                severity: 0,
                domain: 0,
                event: 0,
                arg0: 0,
                arg1: 0,
                arg2: 0,
            }; MAX_CRASH_RECORDS],
            next_slot: 0,
            count: 0,
            total_seen: 0,
        }
    }

    pub(crate) fn record(&mut self, entry: CrashRecord) {
        self.total_seen = self.total_seen.saturating_add(1);
        if self.count < MAX_CRASH_RECORDS {
            self.count += 1;
        }
        self.records[self.next_slot] = entry;
        self.next_slot = (self.next_slot + 1) % MAX_CRASH_RECORDS;
    }

    pub(crate) const fn len(&self) -> usize {
        self.count
    }

    pub(crate) const fn total_seen(&self) -> u64 {
        self.total_seen
    }

    /// Index 0 is the most recent crash; index len()-1 the oldest retained.
    pub(crate) fn recent(&self, index: usize) -> Option<CrashRecord> {
        if index >= self.count {
            return None;
        }
        let slot = (self.next_slot + MAX_CRASH_RECORDS - 1 - index) % MAX_CRASH_RECORDS;
        Some(self.records[slot])
    }

    pub(crate) fn rebuild<const N: usize>(&mut self, records: &[CrashRecord; N], count: usize) {
        self.records = [CrashRecord {
            log_sequence: 0,
            tick: 0,
            source: 0,
            severity: 0,
            domain: 0,
            event: 0,
            arg0: 0,
            arg1: 0,
            arg2: 0,
        }; MAX_CRASH_RECORDS];
        self.next_slot = 0;
        self.count = 0;
        self.total_seen = 0;
        for record in records[..count.min(N)].iter().copied() {
            if CrashRecord::is_crash(record.severity, record.event) {
                self.record(record);
            }
        }
    }
}

/// Builds a crash-query reply. `index` counts backward from the most recent
/// crash (0). Returns the reply tag payload via `reply`.
pub(crate) fn build_query_reply(log: &CrashLog, index: u64, reply: &mut RawMessage) {
    *reply = RawMessage::empty(CRASH_QUERY_REPLY_TAG);
    reply.word_count = CRASH_QUERY_REPLY_WORDS as u32;
    reply.words[0] = CRASH_QUERY_NOT_FOUND;
    reply.words[1] = log.total_seen();
    reply.words[2] = index;
    if let Some(record) = index.try_into().ok().and_then(|i| log.recent(i)) {
        reply.words[0] = CRASH_QUERY_OK;
        record.fill_reply(&mut reply.words);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ERROR_SEV: u32 = LogSeverity::Error as u32;
    const INFO_SEV: u32 = LogSeverity::Info as u32;
    const TRAP_EVENT: u32 = LogEvent::KernelTrap as u32;
    const FAILED_EVENT: u32 = LogEvent::ServiceFailed as u32;
    const STARTED_EVENT: u32 = LogEvent::ServiceStarted as u32;

    fn sample(sequence: u64, severity: u32, event: u32) -> CrashRecord {
        CrashRecord {
            log_sequence: sequence,
            tick: sequence * 10,
            source: 7,
            severity,
            domain: 21,
            event,
            arg0: sequence,
            arg1: 0,
            arg2: 0,
        }
    }

    #[test]
    fn detects_crash_shapes() {
        assert!(CrashRecord::is_crash(ERROR_SEV, STARTED_EVENT));
        assert!(CrashRecord::is_crash(INFO_SEV, TRAP_EVENT));
        assert!(CrashRecord::is_crash(INFO_SEV, FAILED_EVENT));
        assert!(!CrashRecord::is_crash(INFO_SEV, STARTED_EVENT));
    }

    #[test]
    fn recent_returns_newest_first() {
        let mut log = CrashLog::new();
        log.record(sample(1, ERROR_SEV, STARTED_EVENT));
        log.record(sample(2, INFO_SEV, TRAP_EVENT));
        assert_eq!(log.len(), 2);
        assert_eq!(log.recent(0).unwrap().log_sequence, 2);
        assert_eq!(log.recent(1).unwrap().log_sequence, 1);
        assert!(log.recent(2).is_none());
    }

    #[test]
    fn ring_evicts_oldest() {
        let mut log = CrashLog::new();
        for sequence in 0..(MAX_CRASH_RECORDS as u64 + 3) {
            log.record(sample(sequence, ERROR_SEV, STARTED_EVENT));
        }
        assert_eq!(log.len(), MAX_CRASH_RECORDS);
        assert_eq!(
            log.recent(0).unwrap().log_sequence,
            MAX_CRASH_RECORDS as u64 + 2
        );
        assert_eq!(log.recent(MAX_CRASH_RECORDS - 1).unwrap().log_sequence, 3);
        assert!(log.recent(MAX_CRASH_RECORDS).is_none());
        assert_eq!(log.total_seen(), MAX_CRASH_RECORDS as u64 + 3);
    }

    #[test]
    fn rebuild_keeps_only_crashes_in_order() {
        let mut ring = [sample(0, INFO_SEV, STARTED_EVENT); 8];
        ring[0] = sample(10, ERROR_SEV, STARTED_EVENT);
        ring[2] = sample(12, INFO_SEV, TRAP_EVENT);
        ring[5] = sample(15, ERROR_SEV, STARTED_EVENT);
        ring[6] = sample(16, INFO_SEV, STARTED_EVENT);

        let mut log = CrashLog::new();
        log.rebuild(&ring, 8);
        assert_eq!(log.len(), 3);
        assert_eq!(log.recent(0).unwrap().log_sequence, 15);
        assert_eq!(log.recent(1).unwrap().log_sequence, 12);
        assert_eq!(log.recent(2).unwrap().log_sequence, 10);
    }

    #[test]
    fn rebuild_handles_count_over_length() {
        let mut ring = [sample(0, INFO_SEV, STARTED_EVENT); 4];
        ring[1] = sample(9, ERROR_SEV, STARTED_EVENT);
        let mut log = CrashLog::new();
        log.rebuild(&ring, 400);
        assert_eq!(log.len(), 1);
        assert_eq!(log.recent(0).unwrap().log_sequence, 9);
    }

    #[test]
    fn query_reply_encodes_hit_and_miss() {
        let mut log = CrashLog::new();
        log.record(sample(41, ERROR_SEV, STARTED_EVENT));

        let mut reply = RawMessage::empty(0);
        build_query_reply(&log, 0, &mut reply);
        assert_eq!(reply.tag, CRASH_QUERY_REPLY_TAG);
        assert_eq!(reply.words[0], CRASH_QUERY_OK);
        assert_eq!(reply.words[1], 1);
        assert_eq!(reply.words[2], 0);
        assert_eq!(reply.words[3], 41);
        assert_eq!(reply.word_count as usize, CRASH_QUERY_REPLY_WORDS);
        assert!(CRASH_QUERY_REPLY_WORDS <= 16);

        build_query_reply(&log, 1, &mut reply);
        assert_eq!(reply.words[0], CRASH_QUERY_NOT_FOUND);

        let empty = CrashLog::new();
        build_query_reply(&empty, 0, &mut reply);
        assert_eq!(reply.words[0], CRASH_QUERY_NOT_FOUND);
        assert_eq!(reply.words[1], 0);
    }
}
