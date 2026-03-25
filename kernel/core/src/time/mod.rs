#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct MonotonicInstant(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct TimerId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimerDeadline {
    pub deadline: MonotonicInstant,
    pub periodic: bool,
}

pub trait ClockSource {
    fn now(&self) -> MonotonicInstant;
}

pub trait TimerService {
    fn arm(&mut self, deadline: TimerDeadline) -> Result<TimerId, TimerError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimerError {
    UnsupportedInPhase0,
    CapacityExceeded,
}
