use alloc::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
    vec::Vec,
};
use spin::{Mutex, Once};

use crate::{
    capability::{CapabilityRights, CapabilitySpace},
    object::{KernelObjectModel, KernelObjectRef, ObjectId},
    time::{self, MonotonicInstant, TimerRequest, TimerService, WakeEvent, WakeToken},
    user::TaskExitStatus,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct AddressSpaceId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct TaskId(pub u64);

pub type ProcessId = TaskId;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct ThreadId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskRole {
    BootstrapRoot,
    SystemService,
    DriverHost,
    UserService,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThreadMode {
    Kernel,
    User,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionState {
    Constructing,
    Runnable,
    Running,
    Blocked,
    Suspended,
    Dying,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThreadWakeReason {
    Bootstrap,
    Yield,
    TimerExpired,
    ChannelMessage,
    PacketReady,
    InputReady,
    EventSignal,
    Explicit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaitTarget {
    ChannelReceive {
        endpoint: ObjectId,
    },
    ChannelSend {
        endpoint: ObjectId,
    },
    Reply {
        endpoint: ObjectId,
    },
    PacketReceive {
        interface: ObjectId,
    },
    InputReceive {
        source: ObjectId,
    },
    Timer {
        token: WakeToken,
        deadline: MonotonicInstant,
    },
    Event {
        object: ObjectId,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulingContext {
    pub quantum_ticks: u32,
}

impl SchedulingContext {
    pub const fn round_robin_default() -> Self {
        Self { quantum_ticks: 1 }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskDescriptor {
    pub address_space: Option<AddressSpaceId>,
    pub role: TaskRole,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ThreadDescriptor {
    pub mode: ThreadMode,
    pub scheduling_context: SchedulingContext,
    pub entry_instruction_pointer: Option<u64>,
    pub stack_pointer: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskStateView {
    pub id: TaskId,
    pub role: TaskRole,
    pub address_space: Option<AddressSpaceId>,
    pub thread_count: usize,
    pub exit_status: TaskExitStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ThreadStateView {
    pub id: ThreadId,
    pub owner: TaskId,
    pub mode: ThreadMode,
    pub scheduling_context: SchedulingContext,
    pub execution_state: ExecutionState,
    pub wait_target: Option<WaitTarget>,
    pub last_wake_reason: Option<ThreadWakeReason>,
    pub entry_instruction_pointer: Option<u64>,
    pub stack_pointer: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskCreationError {
    ResourceUnavailable,
    InvalidOwnerTask,
}

pub struct TaskObject {
    id: TaskId,
    capability_space: CapabilitySpace,
    state: Mutex<TaskState>,
}

struct TaskState {
    role: TaskRole,
    address_space: Option<AddressSpaceId>,
    threads: Vec<ObjectId>,
    exit_status: TaskExitStatus,
}

impl TaskObject {
    pub fn new(id: TaskId, descriptor: TaskDescriptor) -> Self {
        Self {
            id,
            capability_space: CapabilitySpace::new(),
            state: Mutex::new(TaskState {
                role: descriptor.role,
                address_space: descriptor.address_space,
                threads: Vec::new(),
                exit_status: TaskExitStatus::Running,
            }),
        }
    }

    pub const fn id(&self) -> TaskId {
        self.id
    }

    pub fn capability_space(&self) -> &CapabilitySpace {
        &self.capability_space
    }

    pub fn role(&self) -> TaskRole {
        self.state.lock().role
    }

    pub fn address_space(&self) -> Option<AddressSpaceId> {
        self.state.lock().address_space
    }

    pub fn set_exit_status(&self, exit_status: TaskExitStatus) {
        self.state.lock().exit_status = exit_status;
    }

    pub fn exit_status(&self) -> TaskExitStatus {
        self.state.lock().exit_status
    }

    pub fn attach_thread(&self, thread: ObjectId) {
        let mut state = self.state.lock();
        if !state.threads.contains(&thread) {
            state.threads.push(thread);
        }
    }

    pub fn snapshot(&self) -> TaskStateView {
        let state = self.state.lock();

        TaskStateView {
            id: self.id,
            role: state.role,
            address_space: state.address_space,
            thread_count: state.threads.len(),
            exit_status: state.exit_status,
        }
    }
}

pub struct ThreadObject {
    id: ThreadId,
    state: Mutex<ThreadState>,
}

struct ThreadState {
    owner: TaskId,
    mode: ThreadMode,
    scheduling_context: SchedulingContext,
    execution_state: ExecutionState,
    wait_target: Option<WaitTarget>,
    last_wake_reason: Option<ThreadWakeReason>,
    entry_instruction_pointer: Option<u64>,
    stack_pointer: Option<u64>,
}

impl ThreadObject {
    pub fn new(id: ThreadId, owner: TaskId, descriptor: ThreadDescriptor) -> Self {
        Self {
            id,
            state: Mutex::new(ThreadState {
                owner,
                mode: descriptor.mode,
                scheduling_context: descriptor.scheduling_context,
                execution_state: ExecutionState::Constructing,
                wait_target: None,
                last_wake_reason: None,
                entry_instruction_pointer: descriptor.entry_instruction_pointer,
                stack_pointer: descriptor.stack_pointer,
            }),
        }
    }

    pub const fn id(&self) -> ThreadId {
        self.id
    }

    pub fn snapshot(&self) -> ThreadStateView {
        let state = self.state.lock();

        ThreadStateView {
            id: self.id,
            owner: state.owner,
            mode: state.mode,
            scheduling_context: state.scheduling_context,
            execution_state: state.execution_state,
            wait_target: state.wait_target,
            last_wake_reason: state.last_wake_reason,
            entry_instruction_pointer: state.entry_instruction_pointer,
            stack_pointer: state.stack_pointer,
        }
    }

    pub fn transition_to(
        &self,
        state: ExecutionState,
        wait_target: Option<WaitTarget>,
        wake_reason: Option<ThreadWakeReason>,
    ) {
        let mut thread_state = self.state.lock();
        thread_state.execution_state = state;
        thread_state.wait_target = wait_target;
        thread_state.last_wake_reason = wake_reason;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScheduleTrigger {
    Bootstrap,
    Yield,
    Blocked,
    TimeWake,
    IpcWake,
    NetworkWake,
    InputWake,
    Explicit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScheduleDecision {
    pub trigger: ScheduleTrigger,
    pub previous: Option<ThreadId>,
    pub next: Option<ThreadId>,
    pub runnable_threads: usize,
    pub blocked_threads: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerSnapshot {
    pub current: Option<ThreadId>,
    pub tracked_threads: usize,
    pub runnable_threads: usize,
    pub blocked_threads: usize,
    pub timer_waits: usize,
    pub channel_receive_waits: usize,
    pub packet_receive_waits: usize,
    pub input_receive_waits: usize,
    pub context_switches: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskSystemSnapshot {
    pub bootstrap_task: TaskId,
    pub bootstrap_thread: ThreadId,
    pub scheduler: SchedulerSnapshot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerError {
    InvalidThread,
    ThreadAlreadyRegistered,
    TimeUnavailable,
    WakeTokenExhausted,
    Timer(time::TimerError),
}

impl From<time::TimerError> for SchedulerError {
    fn from(error: time::TimerError) -> Self {
        Self::Timer(error)
    }
}

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
    fn new(bootstrap_thread: KernelObjectRef) -> Self {
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
        state.current = None;

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
}

pub struct TaskSystem {
    objects: &'static KernelObjectModel,
    bootstrap_task: TaskId,
    bootstrap_thread: KernelObjectRef,
    scheduler: Scheduler,
}

impl TaskSystem {
    fn new(objects: &'static KernelObjectModel) -> Self {
        let bootstrap_task = objects
            .bootstrap_task()
            .task()
            .expect("bootstrap task object")
            .id();
        let bootstrap_thread = objects.registry().create_thread(
            objects.bootstrap_task(),
            ThreadDescriptor {
                mode: ThreadMode::Kernel,
                scheduling_context: SchedulingContext::round_robin_default(),
                entry_instruction_pointer: None,
                stack_pointer: None,
            },
        );
        objects
            .bootstrap_task()
            .task()
            .expect("bootstrap task object")
            .capability_space()
            .install(
                Arc::clone(&bootstrap_thread),
                CapabilityRights::thread(),
                Some(1),
            )
            .expect("bootstrap thread install must not exhaust the capability space");

        Self {
            objects,
            bootstrap_task,
            scheduler: Scheduler::new(Arc::clone(&bootstrap_thread)),
            bootstrap_thread,
        }
    }

    pub fn snapshot(&self) -> TaskSystemSnapshot {
        TaskSystemSnapshot {
            bootstrap_task: self.bootstrap_task,
            bootstrap_thread: self.bootstrap_thread(),
            scheduler: self.scheduler.snapshot(),
        }
    }

    pub fn bootstrap_task(&self) -> TaskId {
        self.bootstrap_task
    }

    pub fn objects(&self) -> &'static KernelObjectModel {
        self.objects
    }

    pub fn bootstrap_thread_ref(&self) -> &KernelObjectRef {
        &self.bootstrap_thread
    }

    pub fn bootstrap_thread(&self) -> ThreadId {
        thread_ref_id(&self.bootstrap_thread)
    }

    pub fn scheduler(&self) -> &Scheduler {
        &self.scheduler
    }

    pub fn current_thread_object(&self) -> Option<KernelObjectRef> {
        let thread_id = self.scheduler.current_thread()?;
        self.objects.registry().lookup(ObjectId(thread_id.0))
    }

    pub fn current_task_object(&self) -> Option<KernelObjectRef> {
        let thread = self.current_thread_object()?;
        let owner = thread.thread()?.snapshot().owner;
        self.objects.registry().lookup(ObjectId(owner.0))
    }

    pub fn handle_time_wakeup(&self, event: WakeEvent) -> Option<ScheduleDecision> {
        self.scheduler.handle_time_wakeup(event)
    }

    pub fn notify_channel_ready(&self, endpoint: ObjectId) -> Option<ScheduleDecision> {
        self.scheduler.notify_channel_ready(endpoint)
    }

    pub fn notify_packet_ready(&self, interface: ObjectId) -> Option<ScheduleDecision> {
        self.scheduler.notify_packet_ready(interface)
    }

    pub fn notify_input_ready(&self, source: ObjectId) -> Option<ScheduleDecision> {
        self.scheduler.notify_input_ready(source)
    }
}

static TASK_SYSTEM: Once<TaskSystem> = Once::new();

pub fn initialize(objects: &'static KernelObjectModel) -> &'static TaskSystem {
    TASK_SYSTEM.call_once(|| TaskSystem::new(objects))
}

pub fn system() -> Option<&'static TaskSystem> {
    TASK_SYSTEM.get()
}

pub fn notify_channel_ready(endpoint: ObjectId) -> Option<ScheduleDecision> {
    system().and_then(|tasks| tasks.notify_channel_ready(endpoint))
}

pub fn notify_packet_ready(interface: ObjectId) -> Option<ScheduleDecision> {
    system().and_then(|tasks| tasks.notify_packet_ready(interface))
}

pub fn notify_input_ready(source: ObjectId) -> Option<ScheduleDecision> {
    system().and_then(|tasks| tasks.notify_input_ready(source))
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

pub trait TaskManager {
    fn root_task(&self) -> Option<TaskId>;
    fn create_address_space(&mut self) -> Result<AddressSpaceId, TaskCreationError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bootstrap::{BootContext, BootMemoryRegion, BootMemoryRegionKind},
        memory::PhysicalAddress,
        object::ObjectRegistry,
        time::{self, TimerSourceInfo, WakeReason},
    };

    fn init_test_time() {
        let _ = time::initialize(TimerSourceInfo { tick_hz: 100 });
    }

    fn test_registry() -> (ObjectRegistry, KernelObjectRef, KernelObjectRef) {
        let registry = ObjectRegistry::new();
        let task = registry.create_bootstrap_root_task();
        let thread = registry.create_thread(
            &task,
            ThreadDescriptor {
                mode: ThreadMode::Kernel,
                scheduling_context: SchedulingContext::round_robin_default(),
                entry_instruction_pointer: None,
                stack_pointer: None,
            },
        );
        (registry, task, thread)
    }

    #[test]
    fn scheduler_wakes_blocked_receiver() {
        let (registry, task, bootstrap_thread) = test_registry();
        let scheduler = Scheduler::new(bootstrap_thread);
        let worker = registry.create_thread(
            &task,
            ThreadDescriptor {
                mode: ThreadMode::Kernel,
                scheduling_context: SchedulingContext::round_robin_default(),
                entry_instruction_pointer: None,
                stack_pointer: None,
            },
        );
        let worker_id = scheduler
            .register_thread(Arc::clone(&worker))
            .expect("worker should register");
        let endpoint = ObjectId(42);

        scheduler
            .make_runnable(worker_id, ThreadWakeReason::Explicit)
            .expect("worker should become runnable");
        let switch = scheduler.yield_current().expect("yield should succeed");
        assert_eq!(switch.next, Some(worker_id));

        let block = scheduler
            .block_current_on_receive(endpoint)
            .expect("blocking receive should succeed");
        assert_eq!(block.previous, Some(worker_id));
        assert_eq!(block.next, Some(ThreadId(2)));

        let wake = scheduler
            .notify_channel_ready(endpoint)
            .expect("receiver wake should produce a decision");
        assert_eq!(wake.trigger, ScheduleTrigger::IpcWake);

        let worker_state = worker.thread().expect("thread object").snapshot();
        assert_eq!(worker_state.execution_state, ExecutionState::Runnable);
        assert_eq!(
            worker_state.last_wake_reason,
            Some(ThreadWakeReason::ChannelMessage)
        );
    }

    #[test]
    fn scheduler_wakes_timer_blocked_thread() {
        init_test_time();

        let (registry, task, bootstrap_thread) = test_registry();
        let scheduler = Scheduler::new(bootstrap_thread);
        let worker = registry.create_thread(
            &task,
            ThreadDescriptor {
                mode: ThreadMode::Kernel,
                scheduling_context: SchedulingContext::round_robin_default(),
                entry_instruction_pointer: None,
                stack_pointer: None,
            },
        );
        let worker_id = scheduler
            .register_thread(Arc::clone(&worker))
            .expect("worker should register");

        scheduler
            .make_runnable(worker_id, ThreadWakeReason::Explicit)
            .expect("worker should become runnable");
        let _ = scheduler.yield_current().expect("yield should succeed");

        let (token, block) = scheduler
            .block_current_until(MonotonicInstant(5))
            .expect("timer block should succeed");
        assert_eq!(block.previous, Some(worker_id));
        assert_eq!(block.next, Some(ThreadId(2)));

        let wake = scheduler
            .handle_time_wakeup(WakeEvent {
                token,
                reason: WakeReason::DeadlineExpired,
            })
            .expect("time wake should produce a decision");
        assert_eq!(wake.trigger, ScheduleTrigger::TimeWake);

        let worker_state = worker.thread().expect("thread object").snapshot();
        assert_eq!(worker_state.execution_state, ExecutionState::Runnable);
        assert_eq!(
            worker_state.last_wake_reason,
            Some(ThreadWakeReason::TimerExpired)
        );
    }

    #[test]
    fn block_current_until_reports_wake_token_exhaustion() {
        init_test_time();

        let (_registry, _task, bootstrap_thread) = test_registry();
        let scheduler = Scheduler::new(bootstrap_thread);
        scheduler.state.lock().next_wake_token = u64::MAX;

        assert_eq!(
            scheduler.block_current_until(MonotonicInstant(1)),
            Err(SchedulerError::WakeTokenExhausted)
        );
    }

    #[test]
    fn boot_context_counts_memory_kinds() {
        let regions = [
            BootMemoryRegion {
                start: PhysicalAddress::new(0x1000),
                end: PhysicalAddress::new(0x3000),
                kind: BootMemoryRegionKind::Usable,
            },
            BootMemoryRegion {
                start: PhysicalAddress::new(0x3000),
                end: PhysicalAddress::new(0x4000),
                kind: BootMemoryRegionKind::BootServicesReclaimable,
            },
        ];
        let context = BootContext {
            memory_regions: &regions,
            memory_map_available: true,
            memory_map_truncated: false,
            physical_memory_offset: None,
            rsdp_address: None,
            framebuffer: None,
            boot_store: None,
        };

        assert_eq!(context.usable_memory_region_count(), 1);
        assert_eq!(context.boot_services_reclaimable_region_count(), 1);
    }
}
