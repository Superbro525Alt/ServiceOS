use alloc::vec::Vec;
use spin::Mutex;

use crate::{capability::CapabilitySpace, object::ObjectId};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct AddressSpaceId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct TaskId(pub u64);

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
pub enum ExecutionState {
    Constructing,
    Runnable,
    Running,
    Blocked,
    Suspended,
    Dying,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaitTarget {
    ChannelReceive,
    ChannelSend,
    Reply,
    Timer,
    Event,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulingContext {
    pub budget_ticks: u64,
    pub period_ticks: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskDescriptor {
    pub address_space: Option<AddressSpaceId>,
    pub role: TaskRole,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ThreadDescriptor {
    pub entry_instruction_pointer: Option<u64>,
    pub stack_pointer: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskStateView {
    pub id: TaskId,
    pub role: TaskRole,
    pub address_space: Option<AddressSpaceId>,
    pub thread_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ThreadStateView {
    pub id: ThreadId,
    pub owner: TaskId,
    pub execution_state: ExecutionState,
    pub wait_target: Option<WaitTarget>,
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
            }),
        }
    }

    pub const fn id(&self) -> TaskId {
        self.id
    }

    pub fn capability_space(&self) -> &CapabilitySpace {
        &self.capability_space
    }

    pub fn attach_thread(&self, thread: ObjectId) {
        self.state.lock().threads.push(thread);
    }

    pub fn snapshot(&self) -> TaskStateView {
        let state = self.state.lock();

        TaskStateView {
            id: self.id,
            role: state.role,
            address_space: state.address_space,
            thread_count: state.threads.len(),
        }
    }
}

pub struct ThreadObject {
    id: ThreadId,
    state: Mutex<ThreadState>,
}

struct ThreadState {
    owner: TaskId,
    execution_state: ExecutionState,
    wait_target: Option<WaitTarget>,
    entry_instruction_pointer: Option<u64>,
    stack_pointer: Option<u64>,
}

impl ThreadObject {
    pub fn new(id: ThreadId, owner: TaskId, descriptor: ThreadDescriptor) -> Self {
        Self {
            id,
            state: Mutex::new(ThreadState {
                owner,
                execution_state: ExecutionState::Constructing,
                wait_target: None,
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
            execution_state: state.execution_state,
            wait_target: state.wait_target,
            entry_instruction_pointer: state.entry_instruction_pointer,
            stack_pointer: state.stack_pointer,
        }
    }

    pub fn set_execution_state(&self, state: ExecutionState, wait_target: Option<WaitTarget>) {
        let mut thread_state = self.state.lock();
        thread_state.execution_state = state;
        thread_state.wait_target = wait_target;
    }
}

pub trait TaskManager {
    fn root_task(&self) -> Option<TaskId>;
    fn create_address_space(&mut self) -> Result<AddressSpaceId, TaskCreationError>;
}
