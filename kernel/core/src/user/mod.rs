use alloc::collections::BTreeMap;

use crate::memory::{
    EarlyFrameAllocator, Frame, MappingError, MappingFlags, PAGE_SIZE_BYTES, PageMapper,
    PhysicalAddress, VirtualAddress,
};
use crate::{
    capability::{CapabilityError, PreparedTransfer},
    memory,
    object::KernelObjectRef,
    task::{
        self, AddressSpaceId, SchedulingContext, TaskDescriptor, TaskId, TaskRole,
        ThreadDescriptor, ThreadId, ThreadMode, ThreadWakeReason,
    },
};
use spin::{Mutex, Once};

const FLAT_IMAGE_MAGIC: [u8; 8] = *b"SOSUIMG\0";
const FLAT_IMAGE_HEADER_LEN: usize = 48;
const USER_STACK_PAGES: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskExitStatus {
    Running,
    Exited { code: u64 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreparedUserAddressSpace {
    pub page_table_root: PhysicalAddress,
    pub image: LoadedUserImage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UserThreadLaunch {
    pub thread_id: ThreadId,
    pub page_table_root: PhysicalAddress,
    pub entry_point: u64,
    pub user_stack_pointer: u64,
}

#[derive(Clone, Copy)]
pub struct UserArchHooks {
    pub prepare_address_space:
        fn(&[u8]) -> Result<PreparedUserAddressSpace, AddressSpacePreparationError>,
    pub register_thread_launch: fn(UserThreadLaunch),
}

#[derive(Clone)]
pub struct SpawnedUserTask {
    pub task: KernelObjectRef,
    pub thread: KernelObjectRef,
    pub address_space_id: AddressSpaceId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddressSpacePreparationError {
    Mapping(MappingError),
    Load(LoadError),
    NotInitialized,
}

impl From<MappingError> for AddressSpacePreparationError {
    fn from(error: MappingError) -> Self {
        Self::Mapping(error)
    }
}

impl From<LoadError> for AddressSpacePreparationError {
    fn from(error: LoadError) -> Self {
        Self::Load(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpawnError {
    ObjectsUnavailable,
    TasksUnavailable,
    MemoryUnavailable,
    ImageResolverUnavailable,
    ImageNotFound,
    ArchHooksUnavailable,
    AddressSpace(AddressSpacePreparationError),
    Capability(CapabilityError),
    Scheduler(task::SchedulerError),
}

impl From<AddressSpacePreparationError> for SpawnError {
    fn from(error: AddressSpacePreparationError) -> Self {
        Self::AddressSpace(error)
    }
}

impl From<CapabilityError> for SpawnError {
    fn from(error: CapabilityError) -> Self {
        Self::Capability(error)
    }
}

impl From<task::SchedulerError> for SpawnError {
    fn from(error: task::SchedulerError) -> Self {
        Self::Scheduler(error)
    }
}

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

    fn allocate_address_space_id(&self) -> AddressSpaceId {
        let mut state = self.state.lock();
        let id = AddressSpaceId(state.next_address_space_id);
        state.next_address_space_id = state.next_address_space_id.saturating_add(1);
        id
    }

    fn track_task(&self, task_id: TaskId, thread_id: ThreadId) {
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
static ARCH_HOOKS: Once<UserArchHooks> = Once::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FlatImageHeader {
    pub abi_version: u32,
    pub image_base: VirtualAddress,
    pub entry_offset: u64,
    pub code_size: usize,
    pub user_stack_top: VirtualAddress,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadedUserImage {
    pub entry_point: VirtualAddress,
    pub image_base: VirtualAddress,
    pub code_size: usize,
    pub user_stack_top: VirtualAddress,
    pub mapped_stack_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoadError {
    Truncated,
    InvalidMagic,
    UnsupportedAbi,
    UnsupportedHeader,
    AddressAlignment,
    FrameExhausted,
    Mapping(MappingError),
}

impl From<MappingError> for LoadError {
    fn from(error: MappingError) -> Self {
        Self::Mapping(error)
    }
}

pub fn initialize_runtime() -> &'static UserRuntime {
    USER_RUNTIME.call_once(UserRuntime::new)
}

pub fn runtime() -> Option<&'static UserRuntime> {
    USER_RUNTIME.get()
}

pub fn register_image_resolver(resolver: fn(u32) -> Option<&'static [u8]>) {
    let _ = IMAGE_RESOLVER.call_once(|| resolver);
}

pub fn register_arch_hooks(hooks: UserArchHooks) {
    let _ = ARCH_HOOKS.call_once(|| hooks);
}

pub fn spawn_builtin_task(
    image_id: u32,
    role: TaskRole,
    bootstrap_transfer: Option<PreparedTransfer>,
) -> Result<SpawnedUserTask, SpawnError> {
    let objects = crate::object::model().ok_or(SpawnError::ObjectsUnavailable)?;
    let tasks = task::system().ok_or(SpawnError::TasksUnavailable)?;
    let _memory = memory::manager().ok_or(SpawnError::MemoryUnavailable)?;
    let runtime = initialize_runtime();
    let resolver = IMAGE_RESOLVER
        .get()
        .copied()
        .ok_or(SpawnError::ImageResolverUnavailable)?;
    let hooks = ARCH_HOOKS
        .get()
        .copied()
        .ok_or(SpawnError::ArchHooksUnavailable)?;
    let image = resolver(image_id).ok_or(SpawnError::ImageNotFound)?;
    let address_space_id = runtime.allocate_address_space_id();
    let prepared = (hooks.prepare_address_space)(image)?;

    let task = objects.registry().create_task(TaskDescriptor {
        address_space: Some(address_space_id),
        role,
    });
    if let Some(transfer) = bootstrap_transfer {
        let _ = task
            .task()
            .expect("spawned task object")
            .capability_space()
            .accept_transfer(transfer)?;
    }
    let thread = objects.registry().create_thread(
        &task,
        ThreadDescriptor {
            mode: ThreadMode::User,
            scheduling_context: SchedulingContext::round_robin_default(),
            entry_instruction_pointer: Some(prepared.image.entry_point.as_u64()),
            stack_pointer: Some(prepared.image.user_stack_top.as_u64()),
        },
    );
    let thread_id = thread.thread().expect("spawned thread object").id();

    runtime.track_task(task.task().expect("spawned task object").id(), thread_id);
    (hooks.register_thread_launch)(UserThreadLaunch {
        thread_id,
        page_table_root: prepared.page_table_root,
        entry_point: prepared.image.entry_point.as_u64(),
        user_stack_pointer: prepared.image.user_stack_top.as_u64(),
    });
    tasks.scheduler().register_thread(thread.clone())?;
    tasks
        .scheduler()
        .make_runnable(thread_id, ThreadWakeReason::Explicit)?;

    Ok(SpawnedUserTask {
        task,
        thread,
        address_space_id,
    })
}

pub fn mark_current_thread_exited(code: u64) {
    let Some(tasks) = task::system() else {
        return;
    };
    let Some(thread_id) = tasks.scheduler().current_thread() else {
        return;
    };
    if let Some(runtime) = runtime() {
        runtime.mark_thread_exit(thread_id, code);
    }
    if let Some(task_object) = tasks.current_task_object() {
        if let Some(task) = task_object.task() {
            task.set_exit_status(TaskExitStatus::Exited { code });
        }
    }
}

pub fn current_task_role() -> Option<TaskRole> {
    task::system()
        .and_then(|tasks| tasks.current_task_object())
        .and_then(|object| object.task().map(|task| task.role()))
}

pub fn current_task() -> Option<KernelObjectRef> {
    task::system().and_then(|tasks| tasks.current_task_object())
}

pub fn parse_flat_image(image: &[u8]) -> Result<FlatImageHeader, LoadError> {
    if image.len() < FLAT_IMAGE_HEADER_LEN {
        return Err(LoadError::Truncated);
    }

    if image[..FLAT_IMAGE_MAGIC.len()] != FLAT_IMAGE_MAGIC {
        return Err(LoadError::InvalidMagic);
    }

    let abi_version = read_u32_le(image, 8)?;
    let header_len = read_u32_le(image, 12)? as usize;
    if abi_version != 1 {
        return Err(LoadError::UnsupportedAbi);
    }
    if header_len != FLAT_IMAGE_HEADER_LEN {
        return Err(LoadError::UnsupportedHeader);
    }

    let image_base = VirtualAddress::new(read_u64_le(image, 16)?);
    let entry_offset = read_u64_le(image, 24)?;
    let code_size = read_u64_le(image, 32)? as usize;
    let user_stack_top = VirtualAddress::new(read_u64_le(image, 40)?);

    if image_base.as_u64() % PAGE_SIZE_BYTES != 0 || user_stack_top.as_u64() % PAGE_SIZE_BYTES != 0
    {
        return Err(LoadError::AddressAlignment);
    }
    if image.len() < header_len + code_size {
        return Err(LoadError::Truncated);
    }

    Ok(FlatImageHeader {
        abi_version,
        image_base,
        entry_offset,
        code_size,
        user_stack_top,
    })
}

pub fn load_flat_image(
    image: &[u8],
    mapper: &mut impl PageMapper,
    frame_allocator: &mut EarlyFrameAllocator,
) -> Result<LoadedUserImage, LoadError> {
    let header = parse_flat_image(image)?;
    let code = &image[FLAT_IMAGE_HEADER_LEN..FLAT_IMAGE_HEADER_LEN + header.code_size];

    for (page_index, chunk) in code.chunks(PAGE_SIZE_BYTES as usize).enumerate() {
        let frame = allocate_zeroed_frame(frame_allocator)?;
        copy_into_frame(frame.base, chunk);
        mapper.map_page(
            header
                .image_base
                .offset((page_index as u64) * PAGE_SIZE_BYTES),
            frame,
            MappingFlags::EXECUTABLE | MappingFlags::USER_ACCESSIBLE,
            frame_allocator,
        )?;
    }

    let stack_bottom = VirtualAddress::new(
        header.user_stack_top.as_u64() - ((USER_STACK_PAGES as u64) * PAGE_SIZE_BYTES),
    );
    for page_index in 0..USER_STACK_PAGES {
        let frame = allocate_zeroed_frame(frame_allocator)?;
        mapper.map_page(
            stack_bottom.offset((page_index as u64) * PAGE_SIZE_BYTES),
            frame,
            MappingFlags::WRITABLE | MappingFlags::USER_ACCESSIBLE,
            frame_allocator,
        )?;
    }

    Ok(LoadedUserImage {
        entry_point: header.image_base.offset(header.entry_offset),
        image_base: header.image_base,
        code_size: header.code_size,
        user_stack_top: header.user_stack_top,
        mapped_stack_bytes: USER_STACK_PAGES * PAGE_SIZE_BYTES as usize,
    })
}

fn allocate_zeroed_frame(frame_allocator: &mut EarlyFrameAllocator) -> Result<Frame, LoadError> {
    let frame = frame_allocator
        .allocate_4kib()
        .ok_or(LoadError::FrameExhausted)?;
    unsafe {
        core::ptr::write_bytes(frame.base.as_u64() as *mut u8, 0, PAGE_SIZE_BYTES as usize);
    }
    Ok(frame)
}

fn copy_into_frame(frame_base: PhysicalAddress, bytes: &[u8]) {
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), frame_base.as_u64() as *mut u8, bytes.len());
    }
}

fn read_u32_le(image: &[u8], offset: usize) -> Result<u32, LoadError> {
    let bytes = image
        .get(offset..offset + 4)
        .ok_or(LoadError::Truncated)?
        .try_into()
        .map_err(|_| LoadError::Truncated)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64_le(image: &[u8], offset: usize) -> Result<u64, LoadError> {
    let bytes = image
        .get(offset..offset + 8)
        .ok_or(LoadError::Truncated)?
        .try_into()
        .map_err(|_| LoadError::Truncated)?;
    Ok(u64::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn build_image(
        abi_version: u32,
        image_base: u64,
        entry_offset: u64,
        code_size: u64,
        user_stack_top: u64,
        code_bytes: &[u8],
    ) -> Vec<u8> {
        let mut image = Vec::new();
        image.extend_from_slice(&FLAT_IMAGE_MAGIC);
        image.extend_from_slice(&abi_version.to_le_bytes());
        image.extend_from_slice(&(FLAT_IMAGE_HEADER_LEN as u32).to_le_bytes());
        image.extend_from_slice(&image_base.to_le_bytes());
        image.extend_from_slice(&entry_offset.to_le_bytes());
        image.extend_from_slice(&code_size.to_le_bytes());
        image.extend_from_slice(&user_stack_top.to_le_bytes());
        image.extend_from_slice(code_bytes);
        image
    }

    #[test]
    fn parse_flat_image_accepts_valid_header() {
        let image = build_image(
            1,
            0x4000_0000_0000,
            0x20,
            4,
            0x7fff_ffff_f000,
            &[1, 2, 3, 4],
        );

        let header = parse_flat_image(&image).expect("header should parse");
        assert_eq!(header.abi_version, 1);
        assert_eq!(header.image_base, VirtualAddress::new(0x4000_0000_0000));
        assert_eq!(header.entry_offset, 0x20);
        assert_eq!(header.code_size, 4);
        assert_eq!(header.user_stack_top, VirtualAddress::new(0x7fff_ffff_f000));
    }

    #[test]
    fn parse_flat_image_rejects_misaligned_addresses() {
        let image = build_image(1, 0x4000_0000_0001, 0, 4, 0x7fff_ffff_f000, &[1, 2, 3, 4]);
        assert_eq!(parse_flat_image(&image), Err(LoadError::AddressAlignment));

        let image = build_image(1, 0x4000_0000_0000, 0, 4, 0x7fff_ffff_f001, &[1, 2, 3, 4]);
        assert_eq!(parse_flat_image(&image), Err(LoadError::AddressAlignment));
    }

    #[test]
    fn parse_flat_image_rejects_wrong_abi_and_truncation() {
        let unsupported = build_image(2, 0x4000_0000_0000, 0, 4, 0x7fff_ffff_f000, &[1, 2, 3, 4]);
        assert_eq!(
            parse_flat_image(&unsupported),
            Err(LoadError::UnsupportedAbi)
        );

        let truncated = build_image(1, 0x4000_0000_0000, 0, 8, 0x7fff_ffff_f000, &[1, 2, 3, 4]);
        assert_eq!(parse_flat_image(&truncated), Err(LoadError::Truncated));
    }
}
