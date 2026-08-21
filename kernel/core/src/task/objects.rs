use alloc::vec::Vec;
use spin::Mutex;

use crate::{capability::CapabilitySpace, object::ObjectId, user::TaskExitStatus};

use super::{
    AddressSpaceId, ExecutionState, KernelContext, SchedulingContext, TaskDescriptor, TaskId,
    TaskRole, TaskStateView, ThreadDescriptor, ThreadId, ThreadMode, ThreadStateView,
    ThreadWakeReason, WaitTarget,
};

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
    kernel_context: Option<KernelContext>,
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
                kernel_context: None,
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

    pub fn kernel_context(&self) -> Option<KernelContext> {
        self.state.lock().kernel_context
    }

    pub fn set_kernel_context(&self, context: KernelContext) {
        self.state.lock().kernel_context = Some(context);
    }
}
