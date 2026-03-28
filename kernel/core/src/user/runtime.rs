use alloc::collections::BTreeMap;
use spin::{Mutex, Once};

use crate::task::{AddressSpaceId, TaskId, ThreadId};

use super::TaskExitStatus;

struct UserRuntimeState {
    next_address_space_id: u64,
    tasks: BTreeMap<TaskId, TaskExitStatus>,
    threads: BTreeMap<ThreadId, TaskId>,
}

pub struct UserRuntime {
    state: Mutex<UserRuntimeState>,
}

impl UserRuntime {
    fn new() -> Self {
        Self {
            state: Mutex::new(UserRuntimeState {
                next_address_space_id: 1,
                tasks: BTreeMap::new(),
                threads: BTreeMap::new(),
            }),
        }
    }

    pub fn allocate_address_space_id(&self) -> AddressSpaceId {
        let mut state = self.state.lock();
        let id = AddressSpaceId(state.next_address_space_id);
        state.next_address_space_id = state.next_address_space_id.saturating_add(1);
        id
    }

    pub fn track_task(&self, task_id: TaskId, thread_id: ThreadId) {
        let mut state = self.state.lock();
        state.tasks.insert(task_id, TaskExitStatus::Running);
        state.threads.insert(thread_id, task_id);
    }

    pub fn task_exit_status(&self, task_id: TaskId) -> Option<TaskExitStatus> {
        self.state.lock().tasks.get(&task_id).copied()
    }

    pub fn mark_thread_exit(&self, thread_id: ThreadId, code: u64) {
        let mut state = self.state.lock();
        let Some(task_id) = state.threads.get(&thread_id).copied() else {
            return;
        };
        state.tasks.insert(task_id, TaskExitStatus::Exited { code });
    }
}

static USER_RUNTIME: Once<UserRuntime> = Once::new();
static IMAGE_RESOLVER: Once<fn(u32) -> Option<&'static [u8]>> = Once::new();
static ARCH_HOOKS: Once<super::UserArchHooks> = Once::new();

pub fn initialize_runtime() -> &'static UserRuntime {
    USER_RUNTIME.call_once(UserRuntime::new)
}

pub fn runtime() -> Option<&'static UserRuntime> {
    USER_RUNTIME.get()
}

pub fn register_image_resolver(resolver: fn(u32) -> Option<&'static [u8]>) {
    let _ = IMAGE_RESOLVER.call_once(|| resolver);
}

pub fn register_arch_hooks(hooks: super::UserArchHooks) {
    let _ = ARCH_HOOKS.call_once(|| hooks);
}

pub fn image_resolver() -> Option<fn(u32) -> Option<&'static [u8]>> {
    IMAGE_RESOLVER.get().copied()
}

pub fn arch_hooks() -> Option<super::UserArchHooks> {
    ARCH_HOOKS.get().copied()
}
