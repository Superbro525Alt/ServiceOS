use alloc::collections::{BTreeMap, VecDeque};
use spin::Mutex;

use crate::{
    object::{KernelObjectRef, ObjectId},
    time::{self, MonotonicInstant, TimerRequest, TimerService, WakeEvent, WakeToken},
};

use super::{
    ExecutionState, ScheduleDecision, ScheduleTrigger, SchedulerError, SchedulerSnapshot, ThreadId,
    ThreadObject, ThreadWakeReason, WaitTarget,
};

#[derive(Clone)]
struct ThreadRecord {
    object: KernelObjectRef,
}

struct SchedulerState {
    current: Option<ThreadId>,
    runnable: VecDeque<ThreadId>,
    threads: BTreeMap<ThreadId, ThreadRecord>,
    waiting_timers: BTreeMap<WakeToken, ThreadId>,
    waiting_receivers: BTreeMap<ObjectId, VecDeque<ThreadId>>,
    waiting_packets: BTreeMap<ObjectId, VecDeque<ThreadId>>,
    waiting_inputs: BTreeMap<ObjectId, VecDeque<ThreadId>>,
    next_wake_token: u64,
    context_switches: u64,
}

pub struct Scheduler {
    state: Mutex<SchedulerState>,
}

impl Scheduler {
    pub(super) fn new(bootstrap_thread: KernelObjectRef) -> Self {
        let bootstrap_id = thread_ref_id(&bootstrap_thread);
        bootstrap_thread
            .thread()
            .expect("bootstrap thread object")
            .transition_to(
                ExecutionState::Running,
                None,
                Some(ThreadWakeReason::Bootstrap),
            );

        let mut threads = BTreeMap::new();
        threads.insert(
            bootstrap_id,
            ThreadRecord {
                object: bootstrap_thread,
            },
        );

        Self {
            state: Mutex::new(SchedulerState {
                current: Some(bootstrap_id),
                runnable: VecDeque::new(),
                threads,
                waiting_timers: BTreeMap::new(),
                waiting_receivers: BTreeMap::new(),
                waiting_packets: BTreeMap::new(),
                waiting_inputs: BTreeMap::new(),
                next_wake_token: 1,
                context_switches: 0,
            }),
        }
    }

    pub fn snapshot(&self) -> SchedulerSnapshot {
        let state = self.state.lock();

        SchedulerSnapshot {
            current: state.current,
            tracked_threads: state.threads.len(),
            runnable_threads: state.runnable.len(),
            blocked_threads: blocked_thread_count(&state),
            timer_waits: state.waiting_timers.len(),
            channel_receive_waits: state.waiting_receivers.values().map(VecDeque::len).sum(),
            packet_receive_waits: state.waiting_packets.values().map(VecDeque::len).sum(),
            input_receive_waits: state.waiting_inputs.values().map(VecDeque::len).sum(),
            context_switches: state.context_switches,
        }
    }

    pub fn register_thread(&self, thread: KernelObjectRef) -> Result<ThreadId, SchedulerError> {
        let id = thread_ref_id(&thread);
        let Some(thread_object) = thread.thread() else {
            return Err(SchedulerError::InvalidThread);
        };
        let mut state = self.state.lock();
        if state.threads.contains_key(&id) {
            return Err(SchedulerError::ThreadAlreadyRegistered);
        }

        thread_object.transition_to(ExecutionState::Suspended, None, None);
        state.threads.insert(id, ThreadRecord { object: thread });
        Ok(id)
    }

    pub fn make_runnable(
        &self,
        thread_id: ThreadId,
        wake_reason: ThreadWakeReason,
    ) -> Result<ScheduleDecision, SchedulerError> {
        let mut state = self.state.lock();
        let previous = state.current;
        let thread = lookup_thread_record(&state, thread_id)?;

        if previous == Some(thread_id) {
            thread
                .object
                .thread()
                .expect("registered thread object")
                .transition_to(ExecutionState::Running, None, Some(wake_reason));
            return Ok(decision(
                &state,
                ScheduleTrigger::Explicit,
                previous,
                previous,
            ));
        }

        remove_from_wait_queues(&mut state, thread_id);
        if !state.runnable.contains(&thread_id) {
            state.runnable.push_back(thread_id);
        }
        thread
            .object
            .thread()
            .expect("registered thread object")
            .transition_to(ExecutionState::Runnable, None, Some(wake_reason));

        if previous.is_none() {
            schedule_next_locked(&mut state, ScheduleTrigger::Explicit, previous)
        } else {
            Ok(decision(
                &state,
                ScheduleTrigger::Explicit,
                previous,
                previous,
            ))
        }
    }

    pub fn yield_current(&self) -> Result<ScheduleDecision, SchedulerError> {
        let mut state = self.state.lock();
        let previous = state.current;
        if let Some(current) = previous {
            let thread = lookup_thread(&state, current)?;
            thread.transition_to(
                ExecutionState::Runnable,
                None,
                Some(ThreadWakeReason::Yield),
            );
            state.runnable.push_back(current);
            state.current = None;
        }

        schedule_next_locked(&mut state, ScheduleTrigger::Yield, previous)
    }

    pub fn block_current_on_receive(
        &self,
        endpoint: ObjectId,
    ) -> Result<ScheduleDecision, SchedulerError> {
        let mut state = self.state.lock();
        let Some(current) = state.current else {
            return Ok(decision(&state, ScheduleTrigger::Blocked, None, None));
        };
        let thread = lookup_thread(&state, current)?;
        thread.transition_to(
            ExecutionState::Blocked,
            Some(WaitTarget::ChannelReceive { endpoint }),
            None,
        );
        state.current = None;
        state
            .waiting_receivers
            .entry(endpoint)
            .or_default()
            .push_back(current);

        schedule_next_locked(&mut state, ScheduleTrigger::Blocked, Some(current))
    }

    pub fn block_current_on_packet_receive(
        &self,
        interface: ObjectId,
    ) -> Result<ScheduleDecision, SchedulerError> {
        let mut state = self.state.lock();
        let Some(current) = state.current else {
            return Ok(decision(&state, ScheduleTrigger::Blocked, None, None));
        };
        let thread = lookup_thread(&state, current)?;
        thread.transition_to(
            ExecutionState::Blocked,
            Some(WaitTarget::PacketReceive { interface }),
            None,
        );
        state.current = None;
        state
            .waiting_packets
            .entry(interface)
            .or_default()
            .push_back(current);

        schedule_next_locked(&mut state, ScheduleTrigger::Blocked, Some(current))
    }

    pub fn block_current_on_input_receive(
        &self,
        source: ObjectId,
    ) -> Result<ScheduleDecision, SchedulerError> {
        let mut state = self.state.lock();
        let Some(current) = state.current else {
            return Ok(decision(&state, ScheduleTrigger::Blocked, None, None));
        };
        let thread = lookup_thread(&state, current)?;
        thread.transition_to(
            ExecutionState::Blocked,
            Some(WaitTarget::InputReceive { source }),
            None,
        );
        state.current = None;
        state
            .waiting_inputs
            .entry(source)
            .or_default()
            .push_back(current);

        schedule_next_locked(&mut state, ScheduleTrigger::Blocked, Some(current))
    }

    pub fn block_current_until(
        &self,
        deadline: MonotonicInstant,
    ) -> Result<(WakeToken, ScheduleDecision), SchedulerError> {
        let manager = time::manager().ok_or(SchedulerError::TimeUnavailable)?;
        let mut state = self.state.lock();
        let Some(current) = state.current else {
            return Ok((
                WakeToken(0),
                decision(&state, ScheduleTrigger::Blocked, None, None),
            ));
        };

        let token = WakeToken(state.next_wake_token);
        state.next_wake_token = state
            .next_wake_token
            .checked_add(1)
            .ok_or(SchedulerError::WakeTokenExhausted)?;
        TimerService::arm_wakeup(manager, token, TimerRequest::one_shot(deadline))?;

        let thread = lookup_thread(&state, current)?;
        thread.transition_to(
            ExecutionState::Blocked,
            Some(WaitTarget::Timer { token, deadline }),
            None,
        );
        state.current = None;
        state.waiting_timers.insert(token, current);

        let decision = schedule_next_locked(&mut state, ScheduleTrigger::Blocked, Some(current))?;
        Ok((token, decision))
    }

    pub fn terminate_current(&self) -> Result<ScheduleDecision, SchedulerError> {
        let mut state = self.state.lock();
        let Some(current) = state.current else {
            return Ok(decision(&state, ScheduleTrigger::Explicit, None, None));
        };

        let thread = lookup_thread(&state, current)?;
        thread.transition_to(
            ExecutionState::Dying,
            None,
            Some(ThreadWakeReason::Explicit),
        );
        remove_from_wait_queues(&mut state, current);
        state.runnable.retain(|thread_id| *thread_id != current);
        state.current = None;
        state.threads.remove(&current);

        schedule_next_locked(&mut state, ScheduleTrigger::Explicit, Some(current))
    }

    pub fn handle_time_wakeup(&self, event: WakeEvent) -> Option<ScheduleDecision> {
        let mut state = self.state.lock();
        let previous = state.current;
        let thread_id = state.waiting_timers.remove(&event.token)?;
        let thread = lookup_thread_record(&state, thread_id).ok()?;
        if !state.runnable.contains(&thread_id) {
            state.runnable.push_back(thread_id);
        }
        thread
            .object
            .thread()
            .expect("registered thread object")
            .transition_to(
                ExecutionState::Runnable,
                None,
                Some(ThreadWakeReason::TimerExpired),
            );

        if previous.is_none() {
            schedule_next_locked(&mut state, ScheduleTrigger::TimeWake, previous).ok()
        } else {
            Some(decision(
                &state,
                ScheduleTrigger::TimeWake,
                previous,
                state.current,
            ))
        }
    }

    pub fn notify_channel_ready(&self, endpoint: ObjectId) -> Option<ScheduleDecision> {
        let mut state = self.state.lock();
        let previous = state.current;
        let waiters = state.waiting_receivers.get_mut(&endpoint)?;
        let thread_id = waiters.pop_front()?;
        if waiters.is_empty() {
            state.waiting_receivers.remove(&endpoint);
        }

        let thread = lookup_thread_record(&state, thread_id).ok()?;
        if !state.runnable.contains(&thread_id) {
            state.runnable.push_back(thread_id);
        }
        thread
            .object
            .thread()
            .expect("registered thread object")
            .transition_to(
                ExecutionState::Runnable,
                None,
                Some(ThreadWakeReason::ChannelMessage),
            );

        if previous.is_none() {
            schedule_next_locked(&mut state, ScheduleTrigger::IpcWake, previous).ok()
        } else {
            Some(decision(
                &state,
                ScheduleTrigger::IpcWake,
                previous,
                state.current,
            ))
        }
    }

    pub fn notify_packet_ready(&self, interface: ObjectId) -> Option<ScheduleDecision> {
        let mut state = self.state.lock();
        let previous = state.current;
        let waiters = state.waiting_packets.get_mut(&interface)?;
        let thread_id = waiters.pop_front()?;
        if waiters.is_empty() {
            state.waiting_packets.remove(&interface);
        }

        let thread = lookup_thread_record(&state, thread_id).ok()?;
        if !state.runnable.contains(&thread_id) {
            state.runnable.push_back(thread_id);
        }
        thread
            .object
            .thread()
            .expect("registered thread object")
            .transition_to(
                ExecutionState::Runnable,
                None,
                Some(ThreadWakeReason::PacketReady),
            );

        if previous.is_none() {
            schedule_next_locked(&mut state, ScheduleTrigger::NetworkWake, previous).ok()
        } else {
            Some(decision(
                &state,
                ScheduleTrigger::NetworkWake,
                previous,
                state.current,
            ))
        }
    }

    pub fn notify_input_ready(&self, source: ObjectId) -> Option<ScheduleDecision> {
        let mut state = self.state.lock();
        let previous = state.current;
        let waiters = state.waiting_inputs.get_mut(&source)?;
        let thread_id = waiters.pop_front()?;
        if waiters.is_empty() {
            state.waiting_inputs.remove(&source);
        }

        let thread = lookup_thread_record(&state, thread_id).ok()?;
        if !state.runnable.contains(&thread_id) {
            state.runnable.push_back(thread_id);
        }
        thread
            .object
            .thread()
            .expect("registered thread object")
            .transition_to(
                ExecutionState::Runnable,
                None,
                Some(ThreadWakeReason::InputReady),
            );

        if previous.is_none() {
            schedule_next_locked(&mut state, ScheduleTrigger::InputWake, previous).ok()
        } else {
            Some(decision(
                &state,
                ScheduleTrigger::InputWake,
                previous,
                state.current,
            ))
        }
    }

    pub fn current_thread(&self) -> Option<ThreadId> {
        self.state.lock().current
    }

    #[cfg(test)]
    pub fn set_next_wake_token_for_test(&self, next_wake_token: u64) {
        self.state.lock().next_wake_token = next_wake_token;
    }
}

fn thread_ref_id(thread: &KernelObjectRef) -> ThreadId {
    thread.thread().expect("thread object").id()
}

fn blocked_thread_count(state: &SchedulerState) -> usize {
    state.waiting_timers.len()
        + state
            .waiting_receivers
            .values()
            .map(VecDeque::len)
            .sum::<usize>()
        + state
            .waiting_packets
            .values()
            .map(VecDeque::len)
            .sum::<usize>()
        + state
            .waiting_inputs
            .values()
            .map(VecDeque::len)
            .sum::<usize>()
}

fn decision(
    state: &SchedulerState,
    trigger: ScheduleTrigger,
    previous: Option<ThreadId>,
    next: Option<ThreadId>,
) -> ScheduleDecision {
    ScheduleDecision {
        trigger,
        previous,
        next,
        runnable_threads: state.runnable.len(),
        blocked_threads: blocked_thread_count(state),
    }
}

fn lookup_thread(
    state: &SchedulerState,
    thread_id: ThreadId,
) -> Result<&ThreadObject, SchedulerError> {
    state
        .threads
        .get(&thread_id)
        .and_then(|record| record.object.thread())
        .ok_or(SchedulerError::InvalidThread)
}

fn lookup_thread_record(
    state: &SchedulerState,
    thread_id: ThreadId,
) -> Result<ThreadRecord, SchedulerError> {
    state
        .threads
        .get(&thread_id)
        .cloned()
        .ok_or(SchedulerError::InvalidThread)
}

fn schedule_next_locked(
    state: &mut SchedulerState,
    trigger: ScheduleTrigger,
    previous: Option<ThreadId>,
) -> Result<ScheduleDecision, SchedulerError> {
    let next = state.runnable.pop_front();
    if let Some(thread_id) = next {
        let thread = lookup_thread(state, thread_id)?;
        thread.transition_to(
            ExecutionState::Running,
            None,
            Some(trigger_to_wake_reason(trigger)),
        );
        state.current = Some(thread_id);
        if previous != Some(thread_id) {
            state.context_switches = state.context_switches.saturating_add(1);
        }
    }

    Ok(decision(state, trigger, previous, next))
}

fn remove_from_wait_queues(state: &mut SchedulerState, thread_id: ThreadId) {
    state
        .waiting_timers
        .retain(|_, waiting| *waiting != thread_id);
    state.waiting_receivers.retain(|_, waiters| {
        waiters.retain(|waiting| *waiting != thread_id);
        !waiters.is_empty()
    });
    state.waiting_packets.retain(|_, waiters| {
        waiters.retain(|waiting| *waiting != thread_id);
        !waiters.is_empty()
    });
    state.waiting_inputs.retain(|_, waiters| {
        waiters.retain(|waiting| *waiting != thread_id);
        !waiters.is_empty()
    });
}

const fn trigger_to_wake_reason(trigger: ScheduleTrigger) -> ThreadWakeReason {
    match trigger {
        ScheduleTrigger::Bootstrap => ThreadWakeReason::Bootstrap,
        ScheduleTrigger::Yield => ThreadWakeReason::Yield,
        ScheduleTrigger::Blocked => ThreadWakeReason::Explicit,
        ScheduleTrigger::TimeWake => ThreadWakeReason::TimerExpired,
        ScheduleTrigger::IpcWake => ThreadWakeReason::ChannelMessage,
        ScheduleTrigger::NetworkWake => ThreadWakeReason::PacketReady,
        ScheduleTrigger::InputWake => ThreadWakeReason::InputReady,
        ScheduleTrigger::Explicit => ThreadWakeReason::Explicit,
    }
}
