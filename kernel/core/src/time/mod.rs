mod manager;
mod types;

pub use manager::{TimeManager, initialize, manager};
pub use types::{
    ClockSource, InitializationError, MAX_ARMED_TIMERS, MAX_READY_WAKEUPS, MonotonicInstant,
    TickOutcome, TimeSnapshot, TimerError, TimerId, TimerRequest, TimerService, TimerSourceInfo,
    WakeEvent, WakeReason, WakeToken,
};

impl ClockSource for TimeManager {
    fn now(&self) -> MonotonicInstant {
        self.now()
    }
}
