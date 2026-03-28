pub const MAX_ARMED_TIMERS: usize = 64;
pub const MAX_READY_WAKEUPS: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct MonotonicInstant(pub u64);

impl MonotonicInstant {
    pub const ZERO: Self = Self(0);

    pub const fn saturating_add(self, ticks: u64) -> Self {
        Self(self.0.saturating_add(ticks))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct TimerId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct WakeToken(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WakeReason {
    DeadlineExpired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WakeEvent {
    pub token: WakeToken,
    pub reason: WakeReason,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimerRequest {
    pub deadline: MonotonicInstant,
    pub interval_ticks: Option<u64>,
}

impl TimerRequest {
    pub const fn one_shot(deadline: MonotonicInstant) -> Self {
        Self {
            deadline,
            interval_ticks: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimerSourceInfo {
    pub tick_hz: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TickOutcome {
    pub now: MonotonicInstant,
    pub expired_timers: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimeSnapshot {
    pub now: MonotonicInstant,
    pub tick_hz: u64,
    pub pending_timers: usize,
    pub ready_wakeups: usize,
}

pub trait ClockSource {
    fn now(&self) -> MonotonicInstant;
}

pub trait TimerService {
    fn arm_wakeup(&self, token: WakeToken, request: TimerRequest) -> Result<TimerId, TimerError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitializationError {
    InvalidTickRate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimerError {
    Unsupported,
    CapacityExceeded,
}
