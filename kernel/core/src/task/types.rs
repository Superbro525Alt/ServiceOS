use crate::{
    object::ObjectId,
    time::{MonotonicInstant, WakeToken},
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

/// Kernel-visible task isolation class, fixed at spawn and read-only
/// afterwards. `Unrestricted` is the pre-isolation behavior for every
/// legacy spawn; `Guest` marks a guest workload and arms the syscall
/// dispatcher's dangerous-call gate. Namespace-style boundary only: no
/// address-space isolation, no CPU/memory accounting beyond OOM charging.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TaskIsolationClass {
    #[default]
    Unrestricted,
    Guest,
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
    ObjectReady,
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
    Object {
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
    /// Kernel-enforced isolation class (read-only after spawn).
    pub isolation: TaskIsolationClass,
    /// Owner-environment id handed over by the launcher (read-only after
    /// spawn); `None` for every spawn that does not declare one.
    pub owner_env: Option<u32>,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScheduleTrigger {
    Bootstrap,
    Yield,
    Blocked,
    TimeWake,
    IpcWake,
    NetworkWake,
    InputWake,
    ObjectWake,
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
    pub object_waits: usize,
    pub context_switches: u64,
    pub preemption_pending: bool,
    /// Total threads moved onto a CPU by work-stealing (all mechanisms).
    pub stolen_threads_total: u64,
    /// Total threads relocated by the periodic push-balance pass.
    pub rebalance_moves_total: u64,
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
    Timer(crate::time::TimerError),
}

impl From<crate::time::TimerError> for SchedulerError {
    fn from(error: crate::time::TimerError) -> Self {
        Self::Timer(error)
    }
}

pub trait TaskManager {
    fn root_task(&self) -> Option<TaskId>;
    fn create_address_space(&mut self) -> Result<AddressSpaceId, TaskCreationError>;
}
