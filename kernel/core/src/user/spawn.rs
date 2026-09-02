use crate::{
    memory,
    object::KernelObjectRef,
    syscall::GuestSyscallAbi,
    task::{
        self, SchedulingContext, TaskDescriptor, TaskRole, ThreadDescriptor, ThreadMode,
        ThreadWakeReason,
    },
};

use super::{
    SpawnError, SpawnedUserTask, TaskExitStatus, UserThreadLaunch,
    runtime::{arch_hooks, image_resolver, initialize_runtime, runtime},
};

/// Spawn attributes for an image launch: syscall ABI plus the additive
/// isolation class/owner-env record. The default is byte-identical to the
/// legacy spawn path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpawnImageAttributes {
    pub syscall_abi: GuestSyscallAbi,
    pub isolation: task::TaskIsolationClass,
    pub owner_env: Option<u32>,
}

impl Default for SpawnImageAttributes {
    fn default() -> Self {
        Self {
            syscall_abi: GuestSyscallAbi::Native,
            isolation: task::TaskIsolationClass::Unrestricted,
            owner_env: None,
        }
    }
}

pub fn spawn_builtin_task(
    image_id: u32,
    role: TaskRole,
    bootstrap_transfer: Option<crate::capability::PreparedTransfer>,
) -> Result<SpawnedUserTask, SpawnError> {
    let resolver = image_resolver().ok_or(SpawnError::ImageResolverUnavailable)?;
    let image = resolver(image_id).ok_or(SpawnError::ImageNotFound)?;
    spawn_image_bytes(image, role, bootstrap_transfer)
}

pub fn spawn_image_bytes(
    image: &[u8],
    role: TaskRole,
    bootstrap_transfer: Option<crate::capability::PreparedTransfer>,
) -> Result<SpawnedUserTask, SpawnError> {
    spawn_image_bytes_with_attributes(
        image,
        role,
        bootstrap_transfer,
        SpawnImageAttributes::default(),
    )
}

/// Spawn a user image whose syscalls enter through `abi`. Native numbering
/// (the default) is byte-identical to the unflagged spawn path.
pub fn spawn_image_bytes_with_abi(
    image: &[u8],
    role: TaskRole,
    bootstrap_transfer: Option<crate::capability::PreparedTransfer>,
    syscall_abi: GuestSyscallAbi,
) -> Result<SpawnedUserTask, SpawnError> {
    spawn_image_bytes_with_attributes(
        image,
        role,
        bootstrap_transfer,
        SpawnImageAttributes {
            syscall_abi,
            ..SpawnImageAttributes::default()
        },
    )
}

/// Spawn with full additive attributes: the syscall ABI selects the number
/// translation mode, the isolation class arms the kernel syscall gate, and
/// the owner-env id is recorded read-only on the task.
pub fn spawn_image_bytes_with_attributes(
    image: &[u8],
    role: TaskRole,
    bootstrap_transfer: Option<crate::capability::PreparedTransfer>,
    attributes: SpawnImageAttributes,
) -> Result<SpawnedUserTask, SpawnError> {
    let objects = crate::object::model().ok_or(SpawnError::ObjectsUnavailable)?;
    let tasks = task::system().ok_or(SpawnError::TasksUnavailable)?;
    let _memory = memory::manager().ok_or(SpawnError::MemoryUnavailable)?;
    let runtime = initialize_runtime();
    let hooks = arch_hooks().ok_or(SpawnError::ArchHooksUnavailable)?;
    let address_space_id = runtime.allocate_address_space_id();
    let prepared = (hooks.prepare_address_space)(image)?;
    runtime.register_address_space(address_space_id, prepared.page_table_root);
    runtime.set_syscall_abi(address_space_id, attributes.syscall_abi);
    (hooks.register_address_space)(address_space_id, prepared.page_table_root);
    crate::user::record_loaded_image(address_space_id, prepared.image);

    let task = objects.registry().create_task(TaskDescriptor {
        address_space: Some(address_space_id),
        role,
        isolation: attributes.isolation,
        owner_env: attributes.owner_env,
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
            stack_pointer: Some(prepared.image.initial_stack_pointer()),
        },
    );
    let thread_id = thread.thread().expect("spawned thread object").id();

    runtime.track_task(task.task().expect("spawned task object").id(), thread_id);
    (hooks.register_thread_launch)(UserThreadLaunch {
        thread_id,
        page_table_root: prepared.page_table_root,
        entry_point: prepared.image.entry_point.as_u64(),
        user_stack_pointer: prepared.image.initial_stack_pointer(),
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
    if let Some(hooks) = arch_hooks() {
        (hooks.release_thread_runtime)(thread_id);
    }
    if let Some(task_object) = tasks.current_task_object() {
        if let Some(task) = task_object.task() {
            task.set_exit_status(TaskExitStatus::Exited { code });
            let _ = task::notify_object_ready(task_object.id());
            if let Some(address_space_id) = task.address_space() {
                if let Some(runtime) = runtime() {
                    runtime.release_address_space(address_space_id);
                }
                if let Some(hooks) = arch_hooks() {
                    (hooks.release_address_space)(address_space_id);
                }
            }
        }
    }
}

pub fn mark_current_thread_faulted(code: u64) {
    let Some(tasks) = task::system() else {
        return;
    };
    let Some(thread_id) = tasks.scheduler().current_thread() else {
        return;
    };
    if let Some(runtime) = runtime() {
        runtime.mark_thread_faulted(thread_id, code);
    }
    if let Some(hooks) = arch_hooks() {
        (hooks.release_thread_runtime)(thread_id);
    }
    if let Some(task_object) = tasks.current_task_object() {
        if let Some(task) = task_object.task() {
            task.set_exit_status(TaskExitStatus::Faulted { code });
            let _ = task::notify_object_ready(task_object.id());
            if let Some(address_space_id) = task.address_space() {
                if let Some(runtime) = runtime() {
                    runtime.release_address_space(address_space_id);
                }
                if let Some(hooks) = arch_hooks() {
                    (hooks.release_address_space)(address_space_id);
                }
            }
        }
    }
}

pub fn current_task() -> Option<KernelObjectRef> {
    task::system().and_then(|tasks| tasks.current_task_object())
}
