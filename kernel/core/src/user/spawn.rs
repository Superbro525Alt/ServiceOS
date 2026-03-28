use crate::{
    memory,
    object::KernelObjectRef,
    task::{
        self, SchedulingContext, TaskDescriptor, TaskRole, ThreadDescriptor, ThreadMode,
        ThreadWakeReason,
    },
};

use super::{
    SpawnError, SpawnedUserTask, TaskExitStatus, UserThreadLaunch,
    runtime::{arch_hooks, image_resolver, initialize_runtime, runtime},
};

pub fn spawn_builtin_task(
    image_id: u32,
    role: TaskRole,
    bootstrap_transfer: Option<crate::capability::PreparedTransfer>,
) -> Result<SpawnedUserTask, SpawnError> {
    let objects = crate::object::model().ok_or(SpawnError::ObjectsUnavailable)?;
    let tasks = task::system().ok_or(SpawnError::TasksUnavailable)?;
    let _memory = memory::manager().ok_or(SpawnError::MemoryUnavailable)?;
    let runtime = initialize_runtime();
    let resolver = image_resolver().ok_or(SpawnError::ImageResolverUnavailable)?;
    let hooks = arch_hooks().ok_or(SpawnError::ArchHooksUnavailable)?;
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
    if let Some(task_object) = tasks.current_task_object() {
        if let Some(task) = task_object.task() {
            task.set_exit_status(TaskExitStatus::Exited { code });
        }
    }
}

pub fn current_task() -> Option<KernelObjectRef> {
    task::system().and_then(|tasks| tasks.current_task_object())
}
