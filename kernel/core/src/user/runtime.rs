use alloc::collections::BTreeMap;
use spin::{Mutex, Once};

use crate::{
    memory::{PAGE_SIZE_BYTES, PhysicalAddress, VirtualAddress},
    task::{AddressSpaceId, TaskId, ThreadId},
};

use super::TaskExitStatus;

const FIRST_SHARED_MAPPING_BASE: u64 = 0x0000_6000_0000_0000;

#[derive(Clone, Copy)]
struct AddressSpaceRuntime {
    root: PhysicalAddress,
    next_mapping_base: VirtualAddress,
}

struct UserRuntimeState {
    next_address_space_id: u64,
    tasks: BTreeMap<TaskId, TaskExitStatus>,
    threads: BTreeMap<ThreadId, TaskId>,
    address_spaces: BTreeMap<AddressSpaceId, AddressSpaceRuntime>,
    loaded_images: BTreeMap<AddressSpaceId, super::LoadedUserImage>,
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
                address_spaces: BTreeMap::new(),
                loaded_images: BTreeMap::new(),
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

    pub fn register_address_space(&self, address_space_id: AddressSpaceId, root: PhysicalAddress) {
        self.state.lock().address_spaces.insert(
            address_space_id,
            AddressSpaceRuntime {
                root,
                next_mapping_base: VirtualAddress::new(FIRST_SHARED_MAPPING_BASE),
            },
        );
    }

    pub fn address_space_root(&self, address_space_id: AddressSpaceId) -> Option<PhysicalAddress> {
        self.state
            .lock()
            .address_spaces
            .get(&address_space_id)
            .map(|entry| entry.root)
    }

    pub fn reserve_mapping_range(
        &self,
        address_space_id: AddressSpaceId,
        size_bytes: usize,
    ) -> Option<VirtualAddress> {
        let size_bytes = size_bytes.max(PAGE_SIZE_BYTES as usize);
        let aligned = size_bytes.div_ceil(PAGE_SIZE_BYTES as usize) * PAGE_SIZE_BYTES as usize;
        let mut state = self.state.lock();
        let entry = state.address_spaces.get_mut(&address_space_id)?;
        let base = entry.next_mapping_base;
        entry.next_mapping_base = entry.next_mapping_base.offset(aligned as u64);
        Some(base)
    }

    /// True when `[start, start + size_bytes)` lies entirely inside the
    /// shared-mapping band this runtime handed out for the address space.
    ///
    /// Image and stack regions live below `FIRST_SHARED_MAPPING_BASE` and all
    /// kernel ranges live above `USER_SPACE_END`, so ranges accepted here can
    /// never name kernel mappings or loader-owned user regions. This is what
    /// lets `memory_unmap`/`memory_protect` refuse to mutate pages the task
    /// never mapped through `memory_map`.
    pub fn contains_reserved_mapping_range(
        &self,
        address_space_id: AddressSpaceId,
        start: u64,
        size_bytes: u64,
    ) -> bool {
        let state = self.state.lock();
        let Some(entry) = state.address_spaces.get(&address_space_id) else {
            return false;
        };
        let band_end = entry.next_mapping_base.as_u64();
        let Some(end) = start.checked_add(size_bytes) else {
            return false;
        };
        start >= FIRST_SHARED_MAPPING_BASE && end <= band_end
    }

    pub fn task_exit_status(&self, task_id: TaskId) -> Option<TaskExitStatus> {
        self.state.lock().tasks.get(&task_id).copied()
    }

    pub fn mark_thread_exit(&self, thread_id: ThreadId, code: u64) {
        let mut state = self.state.lock();
        let Some(task_id) = state.threads.remove(&thread_id) else {
            return;
        };
        state.tasks.insert(task_id, TaskExitStatus::Exited { code });
    }

    pub fn mark_thread_faulted(&self, thread_id: ThreadId, code: u64) {
        let mut state = self.state.lock();
        let Some(task_id) = state.threads.remove(&thread_id) else {
            return;
        };
        state
            .tasks
            .insert(task_id, TaskExitStatus::Faulted { code });
    }

    pub fn release_address_space(&self, address_space_id: AddressSpaceId) {
        let mut state = self.state.lock();
        state.address_spaces.remove(&address_space_id);
        state.loaded_images.remove(&address_space_id);
    }

    pub fn record_loaded_image(
        &self,
        address_space_id: AddressSpaceId,
        image: super::LoadedUserImage,
    ) {
        self.state
            .lock()
            .loaded_images
            .insert(address_space_id, image);
    }

    pub fn loaded_image(&self, address_space_id: AddressSpaceId) -> Option<super::LoadedUserImage> {
        self.state
            .lock()
            .loaded_images
            .get(&address_space_id)
            .copied()
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

pub fn record_loaded_image(address_space_id: AddressSpaceId, image: super::LoadedUserImage) {
    if let Some(runtime) = runtime() {
        runtime.record_loaded_image(address_space_id, image);
    }
}

pub fn loaded_image_for(address_space_id: AddressSpaceId) -> Option<super::LoadedUserImage> {
    runtime().and_then(|runtime| runtime.loaded_image(address_space_id))
}

pub fn arch_hooks() -> Option<super::UserArchHooks> {
    ARCH_HOOKS.get().copied()
}
