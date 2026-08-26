use alloc::collections::{BTreeMap, VecDeque};
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

use crate::{
    object::{KernelObjectRef, ObjectId},
    task::KernelContext,
    time::{self, MonotonicInstant, TimerRequest, TimerService, WakeEvent, WakeToken},
};

use super::{
    ExecutionState, ScheduleDecision, ScheduleTrigger, SchedulerError, SchedulerSnapshot, ThreadId,
    ThreadObject, ThreadWakeReason, WaitTarget,
};

/// Number of per-CPU runnable queues. CPUs beyond this cap share the last
/// queue; steal-on-empty keeps every queue drainable from any CPU.
const RUNNABLE_QUEUE_CPUS: usize = 8;

/// Work-stealing: an idle CPU may take up to this many threads per scan from
/// other CPUs' queues.
const STEAL_BATCH_MAX: usize = 2;

/// A queued thread becomes stealable once it has sat runnable for at least
/// this many consecutive scheduler ticks (~30 ms at the 100 Hz tick).
const STEAL_MIN_IDLE_TICKS: u32 = 3;

/// Push balancing runs every this many ticks (per-CPU tick accounting).
const BALANCE_PERIOD_TICKS: u32 = 64;

/// A queue is over-committed when its depth exceeds this ratio times the
/// average depth across all queues; the excess is pushed to the emptiest.
const BALANCE_OVERCOMMIT_RATIO: usize = 2;

/// Number of participating CPUs for proactive load balancing. Defaults to 1,
/// which disables the steal/balance passes entirely and keeps single-core
/// scheduling byte-identical; the platform registers the real CPU count at
/// SMP bring-up.
static BALANCING_CPU_COUNT: AtomicUsize = AtomicUsize::new(1);

/// Optional sink receiving periodic steal-statistics lines. Registration is
/// the debug gate: without a sink nothing is formatted or emitted.
static STEAL_STATS_EMITTER: Mutex<Option<fn(&StealStatsLine)>> = Mutex::new(None);

/// Interval (in ticks) between steal-statistics emissions once a sink is
/// registered.
const STEAL_STATS_PERIOD_TICKS: u32 = 512;

/// Snapshot of the work-stealing counters for one emission point.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StealStatsLine {
    pub tick: u32,
    pub steal_attempts: u64,
    pub stolen_threads_total: u64,
    pub stolen_per_cpu: [u64; RUNNABLE_QUEUE_CPUS],
    pub rebalance_moves: u64,
    pub queue_depths: [usize; RUNNABLE_QUEUE_CPUS],
}

/// Register the number of CPUs participating in proactive load balancing.
/// Values below 2 disable the steal/balance passes (single-core default).
pub fn register_balancing_cpu_count(count: usize) {
    BALANCING_CPU_COUNT.store(count.clamp(1, RUNNABLE_QUEUE_CPUS), Ordering::SeqCst);
}

/// Register the debug sink that receives periodic steal statistics.
pub fn register_steal_stats_emitter(emitter: fn(&StealStatsLine)) {
    *STEAL_STATS_EMITTER.lock() = Some(emitter);
}

/// Optional hook supplying the calling CPU's index (wired from the arch
/// layer's GS-based per-CPU data). Defaults to CPU 0, which keeps scheduler
/// behavior identical to the single global queue on single-core machines.
static CURRENT_CPU_HOOK: Mutex<Option<fn() -> usize>> = Mutex::new(None);

/// Register the hook used to attribute runnable threads to per-CPU queues.
pub fn register_current_cpu_hook(hook: fn() -> usize) {
    *CURRENT_CPU_HOOK.lock() = Some(hook);
}

fn current_cpu_index() -> usize {
    let hook = CURRENT_CPU_HOOK.lock();
    hook.map_or(0, |hook| (hook)() % RUNNABLE_QUEUE_CPUS)
}

#[derive(Clone)]
struct ThreadRecord {
    object: KernelObjectRef,
}

struct SchedulerState {
    current: Option<ThreadId>,
    runnable_queues: [VecDeque<ThreadId>; RUNNABLE_QUEUE_CPUS],
    /// Consecutive ticks each queued thread has waited without running.
    /// Keys are exactly the queued threads; entries reset on re-enqueue.
    queued_idle_ticks: BTreeMap<ThreadId, u32>,
    /// Threads stolen by each CPU (both threshold steals and steal-on-empty).
    stolen_per_cpu: [u64; RUNNABLE_QUEUE_CPUS],
    steal_attempts: u64,
    rebalance_moves: u64,
    tick_counter: u32,
    /// Round-robin scan start position per CPU for the next steal pass.
    steal_scan_cursor: [usize; RUNNABLE_QUEUE_CPUS],
    threads: BTreeMap<ThreadId, ThreadRecord>,
    waiting_timers: BTreeMap<WakeToken, ThreadId>,
    waiting_receivers: BTreeMap<ObjectId, VecDeque<ThreadId>>,
    waiting_packets: BTreeMap<ObjectId, VecDeque<ThreadId>>,
    waiting_inputs: BTreeMap<ObjectId, VecDeque<ThreadId>>,
    waiting_objects: BTreeMap<ObjectId, VecDeque<ThreadId>>,
    next_wake_token: u64,
    context_switches: u64,
    ticks_remaining: u32,
    preemption_pending: bool,
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
                runnable_queues: [const { VecDeque::new() }; RUNNABLE_QUEUE_CPUS],
                queued_idle_ticks: BTreeMap::new(),
                stolen_per_cpu: [0; RUNNABLE_QUEUE_CPUS],
                steal_attempts: 0,
                rebalance_moves: 0,
                tick_counter: 0,
                steal_scan_cursor: [0; RUNNABLE_QUEUE_CPUS],
                threads,
                waiting_timers: BTreeMap::new(),
                waiting_receivers: BTreeMap::new(),
                waiting_packets: BTreeMap::new(),
                waiting_inputs: BTreeMap::new(),
                waiting_objects: BTreeMap::new(),
                next_wake_token: 1,
                context_switches: 0,
                ticks_remaining: 1,
                preemption_pending: false,
            }),
        }
    }

    pub fn snapshot(&self) -> SchedulerSnapshot {
        let state = self.state.lock();

        SchedulerSnapshot {
            current: state.current,
            tracked_threads: state.threads.len(),
            runnable_threads: state.runnable_len(),
            blocked_threads: blocked_thread_count(&state),
            timer_waits: state.waiting_timers.len(),
            channel_receive_waits: state.waiting_receivers.values().map(VecDeque::len).sum(),
            packet_receive_waits: state.waiting_packets.values().map(VecDeque::len).sum(),
            input_receive_waits: state.waiting_inputs.values().map(VecDeque::len).sum(),
            object_waits: state.waiting_objects.values().map(VecDeque::len).sum(),
            context_switches: state.context_switches,
            preemption_pending: state.preemption_pending,
            stolen_threads_total: state.stolen_per_cpu.iter().sum(),
            rebalance_moves_total: state.rebalance_moves,
        }
    }

    /// Per-CPU steal counters and current queue depths (diagnostics path).
    pub fn steal_stats_line(&self) -> StealStatsLine {
        let state = self.state.lock();
        build_stats_line(&state)
    }

    /// Run one work-stealing pass for the calling CPU: scan other queues in
    /// round-robin order and take up to [`STEAL_BATCH_MAX`] threads whose
    /// consecutive idle ticks exceed the threshold. Returns the count moved.
    pub fn steal_idle_runnables(&self) -> usize {
        let mut state = self.state.lock();
        let cpu = current_cpu_index();
        steal_idle_runnables_locked(&mut state, cpu)
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
        state.push_runnable_if_absent(thread_id);
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
            state.push_runnable_on_current_cpu(current);
            state.current = None;
        }

        schedule_next_locked(&mut state, ScheduleTrigger::Yield, previous)
    }

    pub fn preempt_current_if_needed(&self) -> Result<ScheduleDecision, SchedulerError> {
        let mut state = self.state.lock();
        if !state.preemption_pending {
            return Ok(decision(
                &state,
                ScheduleTrigger::Explicit,
                state.current,
                state.current,
            ));
        }
        let previous = state.current;
        if let Some(current) = previous {
            let thread = lookup_thread(&state, current)?;
            thread.transition_to(
                ExecutionState::Runnable,
                None,
                Some(ThreadWakeReason::Yield),
            );
            state.push_runnable_on_current_cpu(current);
            state.current = None;
        }
        state.preemption_pending = false;
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

    /// Block the current thread until a message arrives on `endpoint` or
    /// `deadline_ticks` elapse (0 blocks indefinitely). The timer token keeps
    /// the deadline armed while the channel wait queue provides the message
    /// wake path; whichever fires first makes the thread runnable again.
    pub fn block_current_on_receive_until(
        &self,
        endpoint: ObjectId,
        deadline_ticks: u64,
    ) -> Result<ScheduleDecision, SchedulerError> {
        if deadline_ticks == 0 {
            return self.block_current_on_receive(endpoint);
        }
        let mut state = self.state.lock();
        let Some(current) = state.current else {
            return Ok(decision(&state, ScheduleTrigger::Blocked, None, None));
        };
        let manager = time::manager().ok_or(SchedulerError::TimeUnavailable)?;
        let now = manager.now();
        let deadline = MonotonicInstant(now.0.saturating_add(deadline_ticks));
        let token = WakeToken(state.next_wake_token);
        state.next_wake_token = state
            .next_wake_token
            .checked_add(1)
            .ok_or(SchedulerError::WakeTokenExhausted)?;
        TimerService::arm_wakeup(manager, token, TimerRequest::one_shot(deadline))?;

        let thread = lookup_thread(&state, current)?;
        thread.transition_to(
            ExecutionState::Blocked,
            Some(WaitTarget::ChannelReceive { endpoint }),
            None,
        );
        state.current = None;
        state.waiting_timers.insert(token, current);
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
        // Lost-wakeup guard: clone the source's latch before taking the state
        // lock (never nest the InputCore sources lock inside scheduler state —
        // the IRQ poll path holds it across notify). Consuming the latch after
        // registering the waiter closes the window where an event lands
        // between the receiver's last queue probe and this block decision: its
        // notify hits the not-yet-registered (empty) waiter list, and without
        // this re-check the thread would sleep until the NEXT physical event.
        let raced_wakeup = crate::input::manager()
            .and_then(|core| core.wakeup_latch(source.0))
            .is_some_and(|latch| latch.swap(false, Ordering::AcqRel));

        let mut state = self.state.lock();
        let Some(current) = state.current else {
            return Ok(decision(&state, ScheduleTrigger::Blocked, None, None));
        };
        let thread = lookup_thread(&state, current)?;

        if raced_wakeup {
            // A wakeup already raced us: stay runnable instead of blocking so
            // the receive loop immediately re-drains the queued event(s).
            thread.transition_to(
                ExecutionState::Runnable,
                None,
                Some(ThreadWakeReason::InputReady),
            );
            state.push_runnable_if_absent(current);
            return schedule_next_locked(&mut state, ScheduleTrigger::InputWake, Some(current));
        }

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

    pub fn block_current_on_object(
        &self,
        object: ObjectId,
    ) -> Result<ScheduleDecision, SchedulerError> {
        let mut state = self.state.lock();
        let Some(current) = state.current else {
            return Ok(decision(&state, ScheduleTrigger::Blocked, None, None));
        };
        let thread = lookup_thread(&state, current)?;
        thread.transition_to(
            ExecutionState::Blocked,
            Some(WaitTarget::Object { object }),
            None,
        );
        state.current = None;
        state
            .waiting_objects
            .entry(object)
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
        state.remove_runnable(current);
        state.current = None;
        state.threads.remove(&current);

        schedule_next_locked(&mut state, ScheduleTrigger::Explicit, Some(current))
    }

    pub fn handle_time_wakeup(&self, event: WakeEvent) -> Option<ScheduleDecision> {
        let mut state = self.state.lock();
        let previous = state.current;
        let thread_id = state.waiting_timers.remove(&event.token)?;
        let thread = lookup_thread_record(&state, thread_id).ok()?;
        let waiting = thread
            .object
            .thread()
            .map(|thread| thread.snapshot().execution_state == ExecutionState::Blocked)
            .unwrap_or(false);
        if !waiting {
            return None;
        }
        state.push_runnable_if_absent(thread_id);
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
        let mut woke_receiver = false;
        let mut queue = state
            .waiting_receivers
            .remove(&endpoint)
            .unwrap_or_default();
        while let Some(thread_id) = queue.pop_front() {
            let Ok(thread) = lookup_thread_record(&state, thread_id) else {
                continue;
            };
            let waiting = thread
                .object
                .thread()
                .map(|thread| thread.snapshot().execution_state == ExecutionState::Blocked)
                .unwrap_or(false);
            if !waiting {
                continue;
            }
            state.push_runnable_if_absent(thread_id);
            thread
                .object
                .thread()
                .expect("registered thread object")
                .transition_to(
                    ExecutionState::Runnable,
                    None,
                    Some(ThreadWakeReason::ChannelMessage),
                );
            woke_receiver = true;
            break;
        }
        if !queue.is_empty() {
            state.waiting_receivers.insert(endpoint, queue);
        }
        let woke_object = wake_object_waiters_locked(&mut state, endpoint)?;
        if !woke_receiver && !woke_object {
            return None;
        }

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
        let mut woke_packet = false;
        let mut remove_waiters = false;
        if let Some(waiters) = state.waiting_packets.get_mut(&interface) {
            if let Some(thread_id) = waiters.pop_front() {
                remove_waiters = waiters.is_empty();

                let thread = lookup_thread_record(&state, thread_id).ok()?;
                state.push_runnable_if_absent(thread_id);
                thread
                    .object
                    .thread()
                    .expect("registered thread object")
                    .transition_to(
                        ExecutionState::Runnable,
                        None,
                        Some(ThreadWakeReason::PacketReady),
                    );
                woke_packet = true;
            }
        }
        if remove_waiters {
            state.waiting_packets.remove(&interface);
        }
        let woke_object = wake_object_waiters_locked(&mut state, interface)?;
        if !woke_packet && !woke_object {
            return None;
        }

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
        let mut woke_input = false;
        let mut remove_waiters = false;
        if let Some(waiters) = state.waiting_inputs.get_mut(&source) {
            if let Some(thread_id) = waiters.pop_front() {
                remove_waiters = waiters.is_empty();

                let thread = lookup_thread_record(&state, thread_id).ok()?;
                state.push_runnable_if_absent(thread_id);
                thread
                    .object
                    .thread()
                    .expect("registered thread object")
                    .transition_to(
                        ExecutionState::Runnable,
                        None,
                        Some(ThreadWakeReason::InputReady),
                    );
                woke_input = true;
            }
        }
        if remove_waiters {
            state.waiting_inputs.remove(&source);
        }
        let woke_object = wake_object_waiters_locked(&mut state, source)?;
        if !woke_input && !woke_object {
            return None;
        }

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

    pub fn notify_object_ready(&self, object: ObjectId) -> Option<ScheduleDecision> {
        let mut state = self.state.lock();
        let previous = state.current;
        let waiters = state.waiting_objects.remove(&object)?;
        for thread_id in waiters {
            let thread = lookup_thread_record(&state, thread_id).ok()?;
            state.push_runnable_if_absent(thread_id);
            thread
                .object
                .thread()
                .expect("registered thread object")
                .transition_to(
                    ExecutionState::Runnable,
                    None,
                    Some(ThreadWakeReason::ObjectReady),
                );
        }

        if previous.is_none() {
            schedule_next_locked(&mut state, ScheduleTrigger::ObjectWake, previous).ok()
        } else {
            Some(decision(
                &state,
                ScheduleTrigger::ObjectWake,
                previous,
                state.current,
            ))
        }
    }

    pub fn handle_tick(&self) {
        let mut state = self.state.lock();
        if state.current.is_none() {
            return;
        }
        if state.ticks_remaining > 0 {
            state.ticks_remaining -= 1;
        }
        if state.ticks_remaining == 0 && state.runnable_len() > 0 {
            state.preemption_pending = true;
        }

        // Work-stealing accounting: age every queued thread one tick, then
        // run the periodic push-balance pass when more than one CPU is
        // participating (single-CPU systems skip it entirely, keeping their
        // scheduling order byte-identical).
        state.tick_counter = state.tick_counter.wrapping_add(1);
        for idle in state.queued_idle_ticks.values_mut() {
            *idle = idle.saturating_add(1);
        }
        let tick = state.tick_counter;
        if BALANCING_CPU_COUNT.load(Ordering::SeqCst) > 1 {
            if tick % BALANCE_PERIOD_TICKS == 0 {
                let cpu = current_cpu_index();
                let _stolen = steal_idle_runnables_locked(&mut state, cpu);
                push_balance_locked(&mut state, cpu);
            }
            if tick % STEAL_STATS_PERIOD_TICKS == 0 {
                if let Some(emitter) = STEAL_STATS_EMITTER.lock().as_ref() {
                    let line = build_stats_line(&state);
                    emitter(&line);
                }
            }
        }
    }

    pub fn consume_preemption(&self) -> bool {
        let mut state = self.state.lock();
        if state.preemption_pending {
            state.preemption_pending = false;
            true
        } else {
            false
        }
    }

    /// Non-consuming check used by interrupt context to decide whether the
    /// interrupted user thread must be preempted immediately.
    pub fn preemption_pending(&self) -> bool {
        self.state.lock().preemption_pending
    }

    pub fn current_thread(&self) -> Option<ThreadId> {
        self.state.lock().current
    }

    /// Distinct owner task ids across every tracked thread. The OOM policy
    /// scans these to build its reclaim-candidate list.
    pub fn tracked_thread_owners(&self) -> alloc::vec::Vec<crate::task::TaskId> {
        let state = self.state.lock();
        let mut owners: alloc::vec::Vec<_> = state
            .threads
            .values()
            .filter_map(|record| record.object.thread().map(|thread| thread.snapshot().owner))
            .collect();
        owners.sort_unstable();
        owners.dedup();
        owners
    }

    pub fn kernel_context_switch_info(
        &self,
    ) -> Option<(ThreadId, Option<KernelContext>, Option<KernelContext>)> {
        let state = self.state.lock();
        let previous = state.current?;
        let next = state.runnable_front()?;

        if previous == *next {
            return None;
        }

        let prev_context = lookup_thread_record(&state, previous)
            .ok()
            .and_then(|t| t.object.thread().and_then(|t| t.kernel_context()));
        let next_context = lookup_thread_record(&state, *next)
            .ok()
            .and_then(|t| t.object.thread().and_then(|t| t.kernel_context()));

        Some((previous, prev_context, next_context))
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
        + state
            .waiting_objects
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
        runnable_threads: state.runnable_len(),
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

impl SchedulerState {
    fn runnable_len(&self) -> usize {
        self.runnable_queues.iter().map(VecDeque::len).sum()
    }

    fn push_runnable_on_current_cpu(&mut self, thread_id: ThreadId) {
        let cpu = current_cpu_index();
        self.runnable_queues[cpu].push_back(thread_id);
        self.queued_idle_ticks.insert(thread_id, 0);
    }

    fn push_runnable_if_absent(&mut self, thread_id: ThreadId) {
        if !self.runnable_contains(thread_id) {
            self.push_runnable_on_current_cpu(thread_id);
        }
    }

    fn runnable_contains(&self, thread_id: ThreadId) -> bool {
        self.runnable_queues
            .iter()
            .any(|queue| queue.contains(&thread_id))
    }

    /// Pop the next thread from the calling CPU's queue first, stealing
    /// from the other queues (lowest index first) when it is empty. The
    /// fallback counts as a steal event for the calling CPU.
    fn pop_runnable_next(&mut self) -> Option<ThreadId> {
        let cpu = current_cpu_index();
        if let Some(thread_id) = self.runnable_queues[cpu].pop_front() {
            self.queued_idle_ticks.remove(&thread_id);
            return Some(thread_id);
        }
        let stolen = self
            .runnable_queues
            .iter_mut()
            .find_map(VecDeque::pop_front);
        if let Some(thread_id) = stolen {
            self.queued_idle_ticks.remove(&thread_id);
            self.stolen_per_cpu[cpu] = self.stolen_per_cpu[cpu].saturating_add(1);
        }
        stolen
    }

    fn runnable_front(&self) -> Option<&ThreadId> {
        let cpu = current_cpu_index();
        if let Some(thread_id) = self.runnable_queues[cpu].front() {
            return Some(thread_id);
        }
        self.runnable_queues.iter().find_map(|queue| queue.front())
    }

    fn remove_runnable(&mut self, thread_id: ThreadId) {
        for queue in &mut self.runnable_queues {
            queue.retain(|queued| *queued != thread_id);
        }
        self.queued_idle_ticks.remove(&thread_id);
    }
}

/// Steal up to [`STEAL_BATCH_MAX`] threads whose consecutive idle ticks
/// exceed the threshold from other CPUs' queues, scanning victims in a
/// round-robin order that advances per scan. Stolen threads are requeued on
/// the stealing CPU with a fresh idle clock. Returns the number moved.
fn steal_idle_runnables_locked(state: &mut SchedulerState, cpu: usize) -> usize {
    state.steal_attempts = state.steal_attempts.saturating_add(1);

    let mut candidates: alloc::vec::Vec<(usize, ThreadId)> = alloc::vec::Vec::new();
    let start = state.steal_scan_cursor[cpu];
    for offset in 1..=RUNNABLE_QUEUE_CPUS {
        if candidates.len() >= STEAL_BATCH_MAX {
            break;
        }
        let victim = (start + offset) % RUNNABLE_QUEUE_CPUS;
        if victim == cpu {
            continue;
        }
        for thread_id in state.runnable_queues[victim].iter().copied() {
            if candidates.len() >= STEAL_BATCH_MAX {
                break;
            }
            let idle = state
                .queued_idle_ticks
                .get(&thread_id)
                .copied()
                .unwrap_or(0);
            if idle >= STEAL_MIN_IDLE_TICKS && thread_stealable(thread_id) {
                candidates.push((victim, thread_id));
            }
        }
    }
    // Rotate the starting position so successive passes try a different
    // victim first and no queue is systematically preferred.
    state.steal_scan_cursor[cpu] = (start + 1) % RUNNABLE_QUEUE_CPUS;

    let mut stolen = 0usize;
    for (source, thread_id) in candidates {
        if let Some(position) = state.runnable_queues[source]
            .iter()
            .position(|queued| *queued == thread_id)
        {
            state.runnable_queues[source].remove(position);
            state.runnable_queues[cpu].push_back(thread_id);
            state.queued_idle_ticks.insert(thread_id, 0);
            state.stolen_per_cpu[cpu] = state.stolen_per_cpu[cpu].saturating_add(1);
            stolen += 1;
        }
    }
    stolen
}

/// Affinity gate for stealing. The kernel does not carry per-thread CPU
/// affinity hints yet (no cpuset field exists on thread descriptors), so
/// every queued thread is stealable; when hints land this is the single
/// place that must consult them before a thread changes queues.
fn thread_stealable(_thread_id: ThreadId) -> bool {
    true
}

/// Push-balance pass: when the busiest queue's depth exceeds the
/// over-commit ratio times the average depth across all queues, move its
/// longest-idle threads to the emptiest queue. Returns the number moved.
fn push_balance_locked(state: &mut SchedulerState, _cpu: usize) -> usize {
    let depths: [usize; RUNNABLE_QUEUE_CPUS] =
        core::array::from_fn(|slot| state.runnable_queues[slot].len());
    let Some((busiest, emptiest, moves)) = rebalance_plan(&depths) else {
        return 0;
    };

    let candidates = select_rebalance_candidates(
        &state.runnable_queues[busiest],
        &state.queued_idle_ticks,
        moves,
    );
    let mut moved = 0usize;
    for thread_id in candidates {
        if let Some(position) = state.runnable_queues[busiest]
            .iter()
            .position(|queued| *queued == thread_id)
        {
            state.runnable_queues[busiest].remove(position);
            state.runnable_queues[emptiest].push_back(thread_id);
            state.queued_idle_ticks.insert(thread_id, 0);
            state.rebalance_moves = state.rebalance_moves.saturating_add(1);
            moved += 1;
        }
    }
    moved
}

/// Split math for one push-balance pass: `(busiest, emptiest, moves)` where
/// `moves` shrinks the busiest queue down to the over-commit limit
/// (`ratio * average`, average taken across every queue slot). `None` when
/// there is nothing meaningful to spread (fewer than two runnable threads or
/// no imbalance beyond the limit).
fn rebalance_plan(depths: &[usize; RUNNABLE_QUEUE_CPUS]) -> Option<(usize, usize, usize)> {
    let total: usize = depths.iter().sum();
    if total < 2 {
        return None;
    }
    let average = total / RUNNABLE_QUEUE_CPUS;
    let limit = BALANCE_OVERCOMMIT_RATIO * average.max(1);

    let mut busiest = 0;
    let mut emptiest = 0;
    for (slot, &depth) in depths.iter().enumerate() {
        if depth > depths[busiest] {
            busiest = slot;
        }
        if depth < depths[emptiest] {
            emptiest = slot;
        }
    }
    if busiest == emptiest || depths[busiest] <= limit {
        return None;
    }
    let moves = depths[busiest].saturating_sub(limit).min(depths[busiest]);
    Some((busiest, emptiest, moves))
}

/// Pick which threads leave a rebalanced queue first: the longest-idle ones
/// go (front-of-queue position breaks ties toward older entries), preserving
/// relative FIFO order among equals.
fn select_rebalance_candidates(
    queue: &VecDeque<ThreadId>,
    idle_ticks: &BTreeMap<ThreadId, u32>,
    count: usize,
) -> alloc::vec::Vec<ThreadId> {
    let mut ranked: alloc::vec::Vec<(u32, usize, ThreadId)> = queue
        .iter()
        .enumerate()
        .map(|(position, thread_id)| {
            (
                idle_ticks.get(thread_id).copied().unwrap_or(0),
                usize::MAX - position,
                *thread_id,
            )
        })
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    ranked.truncate(count);
    ranked
        .into_iter()
        .map(|(_, _, thread_id)| thread_id)
        .collect()
}

fn build_stats_line(state: &SchedulerState) -> StealStatsLine {
    let mut queue_depths = [0usize; RUNNABLE_QUEUE_CPUS];
    for (slot, queue) in state.runnable_queues.iter().enumerate() {
        queue_depths[slot] = queue.len();
    }
    StealStatsLine {
        tick: state.tick_counter,
        steal_attempts: state.steal_attempts,
        stolen_threads_total: state.stolen_per_cpu.iter().sum(),
        stolen_per_cpu: state.stolen_per_cpu,
        rebalance_moves: state.rebalance_moves,
        queue_depths,
    }
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
    let next = state.pop_runnable_next();
    if let Some(thread_id) = next {
        let thread = lookup_thread(state, thread_id)?;
        thread.transition_to(
            ExecutionState::Running,
            None,
            Some(trigger_to_wake_reason(trigger)),
        );
        state.current = Some(thread_id);
        state.ticks_remaining = 1;
        state.preemption_pending = false;
        if previous != Some(thread_id) {
            state.context_switches = state.context_switches.saturating_add(1);
        }
    }

    Ok(decision(state, trigger, previous, next))
}

fn wake_object_waiters_locked(state: &mut SchedulerState, object: ObjectId) -> Option<bool> {
    let Some(waiters) = state.waiting_objects.remove(&object) else {
        return Some(false);
    };
    for thread_id in waiters {
        let thread = lookup_thread_record(state, thread_id).ok()?;
        state.push_runnable_if_absent(thread_id);
        thread
            .object
            .thread()
            .expect("registered thread object")
            .transition_to(
                ExecutionState::Runnable,
                None,
                Some(ThreadWakeReason::ObjectReady),
            );
    }
    Some(true)
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
    state.waiting_objects.retain(|_, waiters| {
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
        ScheduleTrigger::ObjectWake => ThreadWakeReason::ObjectReady,
        ScheduleTrigger::Explicit => ThreadWakeReason::Explicit,
    }
}

#[cfg(test)]
mod steal_tests {
    use super::*;
    use crate::{
        object::ObjectRegistry,
        task::{SchedulingContext, ThreadDescriptor, ThreadMode},
    };

    fn descriptor() -> ThreadDescriptor {
        ThreadDescriptor {
            mode: ThreadMode::Kernel,
            scheduling_context: SchedulingContext::round_robin_default(),
            entry_instruction_pointer: None,
            stack_pointer: None,
        }
    }

    fn make_scheduler(
        worker_count: usize,
    ) -> (ObjectRegistry, Scheduler, alloc::vec::Vec<ThreadId>) {
        let registry = ObjectRegistry::new();
        let task = registry.create_bootstrap_root_task();
        let bootstrap = registry.create_thread(&task, descriptor());
        let scheduler = Scheduler::new(bootstrap);
        let mut workers = alloc::vec::Vec::new();
        for _ in 0..worker_count {
            let thread = registry.create_thread(&task, descriptor());
            workers.push(scheduler.register_thread(thread).expect("register worker"));
        }
        (registry, scheduler, workers)
    }

    #[cfg(test)]
    static TEST_CPU_INDEX: AtomicUsize = AtomicUsize::new(0);

    fn test_cpu_hook() -> usize {
        TEST_CPU_INDEX.load(Ordering::SeqCst)
    }

    fn set_cpu(cpu: usize) {
        TEST_CPU_INDEX.store(cpu, Ordering::SeqCst);
        register_current_cpu_hook(test_cpu_hook);
    }

    fn make_runnable_on_cpu(scheduler: &Scheduler, cpu: usize, thread_id: ThreadId) {
        set_cpu(cpu);
        scheduler
            .make_runnable(thread_id, ThreadWakeReason::Explicit)
            .expect("make runnable");
    }

    /// Consolidated pass: the statics (CPU hook, balancing CPU count) are
    /// process-global, so every scenario runs sequentially inside one test
    /// and restores the defaults at the end.
    #[test]
    fn work_stealing_threshold_batch_round_robin_and_rebalance() {
        // --- threshold gating and batch cap --------------------------------
        let (_registry, scheduler, workers) = make_scheduler(4);
        for worker in &workers {
            make_runnable_on_cpu(&scheduler, 0, *worker);
        }
        assert_eq!(scheduler.snapshot().runnable_threads, 4);
        assert_eq!(scheduler.steal_stats_line().queue_depths[0], 4);

        scheduler.handle_tick();
        scheduler.handle_tick();
        set_cpu(1);
        assert_eq!(
            scheduler.steal_idle_runnables(),
            0,
            "threads below the idle threshold must not be stolen"
        );
        assert_eq!(scheduler.steal_stats_line().steal_attempts, 1);

        scheduler.handle_tick(); // third consecutive idle tick
        assert_eq!(
            scheduler.steal_idle_runnables(),
            STEAL_BATCH_MAX,
            "idle CPU takes at most STEAL_BATCH_MAX threads per scan"
        );
        let stats = scheduler.steal_stats_line();
        assert_eq!(stats.queue_depths[0], 2);
        assert_eq!(stats.queue_depths[1], 2);
        assert_eq!(stats.stolen_per_cpu[1], STEAL_BATCH_MAX as u64);
        assert_eq!(stats.stolen_threads_total, STEAL_BATCH_MAX as u64);

        // Freshly stolen threads reset their idle clock, but the threads
        // still sitting on queue 0 keep aging: the next scan takes them too,
        // and only then is everything fresh (no third consecutive steal).
        assert_eq!(scheduler.steal_idle_runnables(), 2);
        let stats = scheduler.steal_stats_line();
        assert_eq!(stats.queue_depths[0], 0);
        assert_eq!(stats.queue_depths[1], 4);
        assert_eq!(scheduler.steal_idle_runnables(), 0);
        assert_eq!(scheduler.snapshot().stolen_threads_total, 4);

        // --- round-robin victim rotation -----------------------------------
        let (_registry, rr_scheduler, rr_workers) = make_scheduler(4);
        for worker in &rr_workers[0..2] {
            make_runnable_on_cpu(&rr_scheduler, 0, *worker);
        }
        for worker in &rr_workers[2..4] {
            make_runnable_on_cpu(&rr_scheduler, 1, *worker);
        }
        for _ in 0..STEAL_MIN_IDLE_TICKS {
            rr_scheduler.handle_tick();
        }
        set_cpu(7);
        assert_eq!(rr_scheduler.steal_idle_runnables(), 2);
        let depths = rr_scheduler.steal_stats_line().queue_depths;
        assert_eq!(depths[0], 2, "scan starting after CPU 7 hits queue 1 first");
        assert_eq!(depths[1], 0);
        assert_eq!(rr_scheduler.steal_idle_runnables(), 2);
        let depths = rr_scheduler.steal_stats_line().queue_depths;
        assert_eq!(depths[0], 0, "rotated scan reaches the remaining victim");
        assert_eq!(rr_scheduler.steal_stats_line().stolen_per_cpu[7], 4);

        // --- rebalance split math ------------------------------------------
        assert_eq!(rebalance_plan(&[0; RUNNABLE_QUEUE_CPUS]), None);
        assert_eq!(rebalance_plan(&[1, 1, 0, 0, 0, 0, 0, 0]), None);
        assert_eq!(
            rebalance_plan(&[3, 3, 0, 0, 0, 0, 0, 0]),
            Some((0, 2, 1)),
            "each queue above the over-commit limit sheds its excess"
        );
        assert_eq!(rebalance_plan(&[10, 0, 0, 0, 0, 0, 0, 0]), Some((0, 1, 8)));
        assert_eq!(rebalance_plan(&[5, 3, 0, 0, 0, 0, 0, 0]), Some((0, 2, 3)));

        // --- candidate ordering: longest-idle first ------------------------
        let mut queue = VecDeque::new();
        let a = ThreadId(101);
        let b = ThreadId(102);
        let c = ThreadId(103);
        queue.push_back(a);
        queue.push_back(b);
        queue.push_back(c);
        let mut idle = BTreeMap::new();
        idle.insert(a, 5u32);
        idle.insert(b, 9u32);
        idle.insert(c, 9u32);
        assert_eq!(
            select_rebalance_candidates(&queue, &idle, 2),
            alloc::vec![b, c]
        );
        assert_eq!(
            select_rebalance_candidates(&queue, &idle, 99).len(),
            3,
            "count above queue depth moves everything available"
        );

        // --- periodic push balance through handle_tick ----------------------
        register_balancing_cpu_count(RUNNABLE_QUEUE_CPUS);
        let (_registry, bal_scheduler, bal_workers) = make_scheduler(6);
        for worker in &bal_workers {
            make_runnable_on_cpu(&bal_scheduler, 0, *worker);
        }
        for _ in 0..BALANCE_PERIOD_TICKS {
            bal_scheduler.handle_tick();
        }
        let stats = bal_scheduler.steal_stats_line();
        assert_eq!(stats.queue_depths[0], 2, "busiest queue shrinks to limit");
        assert_eq!(
            stats.queue_depths[1], 4,
            "emptiest queue receives the excess"
        );
        assert_eq!(stats.rebalance_moves, 4);
        assert_eq!(bal_scheduler.snapshot().rebalance_moves_total, 4);

        register_balancing_cpu_count(1);
        *CURRENT_CPU_HOOK.lock() = None;
    }

    #[test]
    fn steal_on_empty_counts_as_steal_for_calling_cpu() {
        let (_registry, scheduler, workers) = make_scheduler(2);
        make_runnable_on_cpu(&scheduler, 3, workers[0]);
        set_cpu(4); // own queue empty -> fallback pops from queue 3
        let decision = scheduler
            .block_current_on_receive(ObjectId(77))
            .expect("block bootstrap");
        assert_eq!(decision.next, Some(workers[0]));
        let stats = scheduler.steal_stats_line();
        assert_eq!(stats.stolen_per_cpu[4], 1);
        assert_eq!(stats.queue_depths[3], 0);
        *CURRENT_CPU_HOOK.lock() = None;
    }
}
