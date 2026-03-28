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
