use alloc::vec::Vec;
use spin::{Mutex, Once};

/// Memory-pressure classification thresholds, expressed as permille (‰) of
/// remaining headroom over the domain total (usable frames or heap bytes).
/// A domain at or below [`CRITICAL_HEADROOM_PERMILLE`] classifies Critical,
/// at or below [`TIGHT_HEADROOM_PERMILLE`] classifies Tight, else Normal.
pub const TIGHT_HEADROOM_PERMILLE: u64 = 250;
pub const CRITICAL_HEADROOM_PERMILLE: u64 = 100;

const MAX_RECORDED_TRANSITIONS: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum PressureLevel {
    Normal,
    Tight,
    Critical,
}

impl PressureLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            PressureLevel::Normal => "normal",
            PressureLevel::Tight => "tight",
            PressureLevel::Critical => "critical",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PressureReading {
    /// Free usable frames as permille of total usable frames.
    pub frames_headroom_permille: u64,
    /// Free heap bytes as permille of heap capacity.
    pub heap_headroom_permille: u64,
}

/// Convert a free/total pair into permille of headroom. An untrackable
/// domain (`total == 0`) reports full headroom so it never raises pressure.
pub fn headroom_permille(free: u64, total: u64) -> u64 {
    if total == 0 {
        return 1000;
    }
    free.saturating_mul(1000) / total
}

/// Classify a reading: the worst (lowest-headroom) domain decides the level.
pub fn classify(reading: PressureReading) -> PressureLevel {
    let worst = reading
        .frames_headroom_permille
        .min(reading.heap_headroom_permille);
    if worst <= CRITICAL_HEADROOM_PERMILLE {
        PressureLevel::Critical
    } else if worst <= TIGHT_HEADROOM_PERMILLE {
        PressureLevel::Tight
    } else {
        PressureLevel::Normal
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PressureTransition {
    pub from: PressureLevel,
    pub to: PressureLevel,
    pub tick: u64,
}

struct PressureMonitor {
    current: PressureLevel,
    listeners: Vec<fn(&PressureTransition)>,
    transitions: Vec<PressureTransition>,
}

impl PressureMonitor {
    const fn new() -> Self {
        Self {
            current: PressureLevel::Normal,
            listeners: Vec::new(),
            transitions: Vec::new(),
        }
    }
}

static MONITOR: Once<Mutex<PressureMonitor>> = Once::new();

fn monitor() -> Option<&'static Mutex<PressureMonitor>> {
    MONITOR.get()
}

pub fn initialize() {
    let _ = MONITOR.call_once(|| Mutex::new(PressureMonitor::new()));
}

#[cfg(test)]
pub fn reset_for_tests() {
    if let Some(monitor) = MONITOR.get() {
        *monitor.lock() = PressureMonitor::new();
    }
}

pub fn current_level() -> Option<PressureLevel> {
    monitor().map(|monitor| monitor.lock().current)
}

pub fn register_listener(listener: fn(&PressureTransition)) {
    if let Some(monitor) = monitor() {
        monitor.lock().listeners.push(listener);
    }
}

/// Recorded transitions, oldest first, capped at the ring size.
pub fn transitions_snapshot() -> Vec<PressureTransition> {
    monitor()
        .map(|monitor| monitor.lock().transitions.clone())
        .unwrap_or_default()
}

/// Feed a fresh reading into the monitor. Returns the transition when the
/// classified level changed, after notifying every registered listener and
/// recording the transition in the bounded ring.
pub fn observe(reading: PressureReading, tick: u64) -> Option<PressureTransition> {
    let monitor = monitor()?;
    let next = classify(reading);
    let mut monitor = monitor.lock();
    let previous = monitor.current;
    if next == previous {
        return None;
    }

    monitor.current = next;
    let transition = PressureTransition {
        from: previous,
        to: next,
        tick,
    };
    for listener in &monitor.listeners {
        listener(&transition);
    }
    monitor.transitions.push(transition);
    if monitor.transitions.len() > MAX_RECORDED_TRANSITIONS {
        let excess = monitor.transitions.len() - MAX_RECORDED_TRANSITIONS;
        monitor.transitions.drain(0..excess);
    }
    Some(transition)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};

    static LISTENER_CALLS: AtomicUsize = AtomicUsize::new(0);

    fn listener_seen(_transition: &PressureTransition) {
        LISTENER_CALLS.fetch_add(1, Ordering::SeqCst);
    }

    fn reading(frames: u64, heap: u64) -> PressureReading {
        PressureReading {
            frames_headroom_permille: frames,
            heap_headroom_permille: heap,
        }
    }

    #[test]
    fn classification_transitions_listeners_and_ring_end_to_end() {
        reset_for_tests();
        initialize();

        // --- threshold classification ---------------------------------------
        assert_eq!(classify(reading(1000, 1000)), PressureLevel::Normal);
        assert_eq!(classify(reading(251, 999)), PressureLevel::Normal);
        assert_eq!(classify(reading(250, 999)), PressureLevel::Tight);
        assert_eq!(classify(reading(101, 999)), PressureLevel::Tight);
        assert_eq!(classify(reading(100, 999)), PressureLevel::Critical);
        assert_eq!(classify(reading(0, 999)), PressureLevel::Critical);
        // The worse domain decides: healthy frames cannot mask an empty heap.
        assert_eq!(classify(reading(900, 10)), PressureLevel::Critical);
        assert_eq!(classify(reading(200, 900)), PressureLevel::Tight);

        // --- headroom math ---------------------------------------------------
        assert_eq!(headroom_permille(25, 100), 250);
        assert_eq!(headroom_permille(1, 1000), 1);
        assert_eq!(headroom_permille(7, 0), 1000);
        assert_eq!(headroom_permille(0, 5000), 0);

        // --- monitor: transitions, listeners, ring ---------------------------
        LISTENER_CALLS.store(0, Ordering::SeqCst);
        register_listener(listener_seen);

        assert_eq!(current_level(), Some(PressureLevel::Normal));
        assert_eq!(
            observe(reading(900, 900), 10),
            None,
            "same-level observations produce no transition"
        );
        assert_eq!(
            observe(reading(200, 900), 20),
            Some(PressureTransition {
                from: PressureLevel::Normal,
                to: PressureLevel::Tight,
                tick: 20
            })
        );
        assert_eq!(current_level(), Some(PressureLevel::Tight));
        assert_eq!(
            observe(reading(50, 900), 30),
            Some(PressureTransition {
                from: PressureLevel::Tight,
                to: PressureLevel::Critical,
                tick: 30
            })
        );
        // Recovery is a transition too (operators see pressure lift).
        assert_eq!(
            observe(reading(400, 900), 40),
            Some(PressureTransition {
                from: PressureLevel::Critical,
                to: PressureLevel::Normal,
                tick: 40
            })
        );

        assert_eq!(LISTENER_CALLS.load(Ordering::SeqCst), 3);
        let transitions = transitions_snapshot();
        assert_eq!(transitions.len(), 3);
        assert_eq!(transitions[0].to, PressureLevel::Tight);
        assert_eq!(transitions[2].from, PressureLevel::Critical);

        // --- ring bound -------------------------------------------------------
        for index in 0..(MAX_RECORDED_TRANSITIONS as u64 + 8) {
            let frames = if index % 2 == 0 { 50 } else { 900 };
            observe(reading(frames, 900), 100 + index);
        }
        assert_eq!(transitions_snapshot().len(), MAX_RECORDED_TRANSITIONS);
    }
}
