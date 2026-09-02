use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use spin::{Mutex, Once};

use crate::{
    memory::{PAGE_SIZE_BYTES, PhysicalAddress, VirtualAddress},
    syscall::GuestSyscallAbi,
    task::{AddressSpaceId, TaskId, ThreadId},
};

use super::{TaskExitStatus, TaskSymbolTable};

const FIRST_SHARED_MAPPING_BASE: u64 = 0x0000_6000_0000_0000;

/// Maximum runtime-loaded libraries per task (each `TaskLoadLibrary`
/// call beyond this fails with a capacity error). Spawn-time companion
/// records are tracked separately and do not consume this budget.
pub const MAX_RUNTIME_LIBRARIES: usize = 4;

/// One runtime-loaded library: the handle `TaskLoadLibrary` returned plus
/// the placement facts `TaskLoadedLibraries` reports.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeLibraryRecord {
    pub handle: u32,
    pub base: u64,
    pub mapped_bytes: u64,
}

struct TaskLibraryState {
    next_handle: u32,
    table: TaskSymbolTable,
    libraries: Vec<RuntimeLibraryRecord>,
}

impl TaskLibraryState {
    fn new(seed: TaskSymbolTable) -> Self {
        Self {
            next_handle: 1,
            table: seed,
            libraries: Vec::new(),
        }
    }
}

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
    /// Task-scoped library state: export seed captured at spawn, grown by
    /// every runtime-loaded library. Seeded for every spawned user task.
    task_libraries: BTreeMap<AddressSpaceId, Box<TaskLibraryState>>,
    /// Guest syscall-ABI mode per address space, set only for explicitly
    /// flagged spawns; every unlisted address space is native.
    syscall_abis: BTreeMap<AddressSpaceId, GuestSyscallAbi>,
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
                task_libraries: BTreeMap::new(),
                syscall_abis: BTreeMap::new(),
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
        state.task_libraries.remove(&address_space_id);
        state.syscall_abis.remove(&address_space_id);
    }

    /// Seed the task-scoped library state at spawn: the export table starts
    /// as the spawn image's own exports plus its mapped companions'.
    pub fn seed_library_state(&self, address_space_id: AddressSpaceId, seed: TaskSymbolTable) {
        self.state
            .lock()
            .task_libraries
            .entry(address_space_id)
            .or_insert_with(|| Box::new(TaskLibraryState::new(seed)));
    }

    /// Begin a runtime load: hand out the next library handle and a staging
    /// copy of the task's symbol table. The load completes with
    /// `commit_runtime_load`; a discarded staging copy leaves the task
    /// state untouched. `None` = unknown address space or the per-task
    /// runtime-library budget is exhausted.
    pub fn begin_runtime_load(
        &self,
        address_space_id: AddressSpaceId,
    ) -> Option<(u32, TaskSymbolTable)> {
        let mut state = self.state.lock();
        let entry = state.task_libraries.get_mut(&address_space_id)?;
        if entry.libraries.len() >= MAX_RUNTIME_LIBRARIES {
            return None;
        }
        if entry.next_handle == u32::MAX {
            return None;
        }
        let handle = entry.next_handle;
        entry.next_handle += 1;
        Some((handle, entry.table))
    }

    /// Commit a completed runtime load: swap in the staged symbol table
    /// (seed exports + this library's exports) and record the placement.
    pub fn commit_runtime_load(
        &self,
        address_space_id: AddressSpaceId,
        handle: u32,
        base: u64,
        mapped_bytes: u64,
        table: TaskSymbolTable,
    ) -> bool {
        let mut state = self.state.lock();
        let Some(entry) = state.task_libraries.get_mut(&address_space_id) else {
            return false;
        };
        entry.table = table;
        entry.libraries.push(RuntimeLibraryRecord {
            handle,
            base,
            mapped_bytes,
        });
        true
    }

    /// True when `handle` names a library this task runtime-loaded.
    pub fn runtime_library_known(&self, address_space_id: AddressSpaceId, handle: u32) -> bool {
        self.state
            .lock()
            .task_libraries
            .get(&address_space_id)
            .is_some_and(|entry| entry.libraries.iter().any(|record| record.handle == handle))
    }

    /// Resolve a name against the task's global symbol table (spawn seed
    /// plus every runtime-loaded library's exports).
    pub fn lookup_task_symbol(&self, address_space_id: AddressSpaceId, name: &[u8]) -> Option<u64> {
        self.state
            .lock()
            .task_libraries
            .get(&address_space_id)
            .and_then(|entry| entry.table.lookup(name))
    }

    /// The task's runtime-loaded library records (empty when unseeded).
    pub fn runtime_library_records(
        &self,
        address_space_id: AddressSpaceId,
    ) -> Vec<RuntimeLibraryRecord> {
        self.state
            .lock()
            .task_libraries
            .get(&address_space_id)
            .map(|entry| entry.libraries.clone())
            .unwrap_or_default()
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

    /// Record the syscall ABI a spawned address space enters syscalls
    /// through. Absent entries mean native numbering.
    pub fn set_syscall_abi(&self, address_space_id: AddressSpaceId, abi: GuestSyscallAbi) {
        self.state.lock().syscall_abis.insert(address_space_id, abi);
    }

    pub fn syscall_abi(&self, address_space_id: AddressSpaceId) -> GuestSyscallAbi {
        self.state
            .lock()
            .syscall_abis
            .get(&address_space_id)
            .copied()
            .unwrap_or_default()
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

/// Seed the address space's task-scoped library state with the spawn
/// image's export table (main image + companions). Best-effort: a runtime
/// without a user runtime simply never sees runtime loads.
pub fn seed_library_state(address_space_id: AddressSpaceId, seed: super::TaskSymbolTable) {
    if let Some(runtime) = runtime() {
        runtime.seed_library_state(address_space_id, seed);
    }
}

pub fn loaded_image_for(address_space_id: AddressSpaceId) -> Option<super::LoadedUserImage> {
    runtime().and_then(|runtime| runtime.loaded_image(address_space_id))
}

/// Syscall ABI of the currently running task: `Native` when no user runtime
/// or current task exists (every kernel-internal path stays native).
pub fn current_task_syscall_abi() -> Option<GuestSyscallAbi> {
    let task = crate::task::system()?.current_task_object()?;
    let address_space = task.task()?.address_space()?;
    runtime().map(|runtime| runtime.syscall_abi(address_space))
}

pub fn arch_hooks() -> Option<super::UserArchHooks> {
    ARCH_HOOKS.get().copied()
}
