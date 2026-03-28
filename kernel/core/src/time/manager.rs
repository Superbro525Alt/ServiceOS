use spin::{Mutex, Once};

use super::{
    InitializationError, MAX_ARMED_TIMERS, MAX_READY_WAKEUPS, MonotonicInstant, TickOutcome,
    TimeSnapshot, TimerError, TimerId, TimerRequest, TimerService, TimerSourceInfo, WakeEvent,
    WakeReason, WakeToken,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ArmedTimer {
    id: TimerId,
    token: WakeToken,
    request: TimerRequest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ReadyWakeupRing {
    slots: [Option<WakeEvent>; MAX_READY_WAKEUPS],
}

impl ReadyWakeupRing {
    const fn new() -> Self {
        Self {
            slots: [None; MAX_READY_WAKEUPS],
        }
    }

    fn push(&mut self, event: WakeEvent) -> bool {
        for slot in &mut self.slots {
            if slot.is_none() {
                *slot = Some(event);
                return true;
            }
        }

        false
    }

    fn pop(&mut self) -> Option<WakeEvent> {
        for slot in &mut self.slots {
            if slot.is_some() {
                return slot.take();
            }
        }

        None
    }

    fn len(&self) -> usize {
        self.slots.iter().filter(|slot| slot.is_some()).count()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TimeState {
    source: TimerSourceInfo,
    next_timer_id: u64,
    now: MonotonicInstant,
    timers: [Option<ArmedTimer>; MAX_ARMED_TIMERS],
    ready: ReadyWakeupRing,
}

impl TimeState {
    const fn new(source: TimerSourceInfo) -> Self {
        Self {
            source,
            next_timer_id: 1,
            now: MonotonicInstant::ZERO,
            timers: [None; MAX_ARMED_TIMERS],
            ready: ReadyWakeupRing::new(),
        }
    }

    fn arm_wakeup(
        &mut self,
        token: WakeToken,
        request: TimerRequest,
    ) -> Result<TimerId, TimerError> {
        let id = TimerId(self.next_timer_id);
        self.next_timer_id = self.next_timer_id.saturating_add(1);

        if request.deadline <= self.now {
            if self.ready.push(WakeEvent {
                token,
                reason: WakeReason::DeadlineExpired,
            }) {
                return Ok(id);
            }

            return Err(TimerError::CapacityExceeded);
        }

        for slot in &mut self.timers {
            if slot.is_none() {
                *slot = Some(ArmedTimer { id, token, request });
                return Ok(id);
            }
        }

        Err(TimerError::CapacityExceeded)
    }

    fn handle_tick(&mut self) -> TickOutcome {
        self.now = self.now.saturating_add(1);
        let mut expired_timers = 0usize;

        for slot in &mut self.timers {
            let Some(mut timer) = *slot else {
                continue;
            };

            if timer.request.deadline > self.now {
                continue;
            }

            if !self.ready.push(WakeEvent {
                token: timer.token,
                reason: WakeReason::DeadlineExpired,
            }) {
                continue;
            }

            expired_timers += 1;

            if let Some(interval_ticks) = timer.request.interval_ticks.filter(|ticks| *ticks > 0) {
                timer.request.deadline = self.now.saturating_add(interval_ticks);
                *slot = Some(timer);
            } else {
                *slot = None;
            }
        }

        TickOutcome {
            now: self.now,
            expired_timers,
        }
    }

    fn pending_timers(&self) -> usize {
        self.timers.iter().filter(|slot| slot.is_some()).count()
    }
}

pub struct TimeManager {
    state: Mutex<TimeState>,
}

impl TimeManager {
    fn new(source: TimerSourceInfo) -> Self {
        Self {
            state: Mutex::new(TimeState::new(source)),
        }
    }

    pub fn source(&self) -> TimerSourceInfo {
        self.state.lock().source
    }

    pub fn now(&self) -> MonotonicInstant {
        self.state.lock().now
    }

    pub fn handle_tick(&self) -> TickOutcome {
        self.state.lock().handle_tick()
    }

    pub fn take_wakeup(&self) -> Option<WakeEvent> {
        self.state.lock().ready.pop()
    }

    pub fn snapshot(&self) -> TimeSnapshot {
        let state = self.state.lock();

        TimeSnapshot {
            now: state.now,
            tick_hz: state.source.tick_hz,
            pending_timers: state.pending_timers(),
            ready_wakeups: state.ready.len(),
        }
    }
}

impl TimerService for TimeManager {
    fn arm_wakeup(&self, token: WakeToken, request: TimerRequest) -> Result<TimerId, TimerError> {
        self.state.lock().arm_wakeup(token, request)
    }
}

static TIME_MANAGER: Once<TimeManager> = Once::new();

pub fn initialize(source: TimerSourceInfo) -> Result<&'static TimeManager, InitializationError> {
    if source.tick_hz == 0 {
        return Err(InitializationError::InvalidTickRate);
    }

    Ok(TIME_MANAGER.call_once(|| TimeManager::new(source)))
}

pub fn manager() -> Option<&'static TimeManager> {
    TIME_MANAGER.get()
}
