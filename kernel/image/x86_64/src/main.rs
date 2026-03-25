#![no_main]
#![no_std]

extern crate alloc;

use alloc::sync::Arc;
use core::panic::PanicInfo;
use serviceos_kernel_arch_x86_64::{
    boot::exit_boot_services_and_capture_context,
    cpu,
    interrupts::{self, TIMER_TICK_HZ},
    paging::{ActivePageTable, OwnedPageTable},
    serial, user,
};
use serviceos_kernel_core::{
    Kernel,
    capability::{CapabilityError, CapabilityRights},
    memory::MappingError,
    object::ObjectId,
    task::{
        AddressSpaceId, ScheduleDecision, SchedulerError, SchedulingContext, TaskDescriptor,
        TaskRole, ThreadDescriptor, ThreadId, ThreadMode,
    },
    user::{LoadError, LoadedUserImage},
};
use serviceos_userspace_demo_x86_64 as userspace_demo;
use uefi::{Status, entry};

enum Phase5BootError {
    MissingTaskObject,
    MissingThreadObject,
    UnexpectedSchedule {
        expected: ThreadId,
        actual: Option<ThreadId>,
    },
    Capability(CapabilityError),
    Scheduler(SchedulerError),
    Mapping(MappingError),
    Load(LoadError),
    UserLaunch(user::UserLaunchError),
}

impl core::fmt::Display for Phase5BootError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::MissingTaskObject => formatter.write_str("missing task object"),
            Self::MissingThreadObject => formatter.write_str("missing thread object"),
            Self::UnexpectedSchedule { expected, actual } => write!(
                formatter,
                "unexpected schedule outcome: expected thread {} got {:?}",
                expected.0, actual
            ),
            Self::Capability(error) => write!(formatter, "capability error: {error:?}"),
            Self::Scheduler(error) => write!(formatter, "scheduler error: {error:?}"),
            Self::Mapping(error) => write!(formatter, "page table mapping error: {error:?}"),
            Self::Load(error) => write!(formatter, "user image load error: {error:?}"),
            Self::UserLaunch(error) => write!(formatter, "user launch error: {error:?}"),
        }
    }
}

impl From<CapabilityError> for Phase5BootError {
    fn from(error: CapabilityError) -> Self {
        Self::Capability(error)
    }
}

impl From<SchedulerError> for Phase5BootError {
    fn from(error: SchedulerError) -> Self {
        Self::Scheduler(error)
    }
}

impl From<MappingError> for Phase5BootError {
    fn from(error: MappingError) -> Self {
        Self::Mapping(error)
    }
}

impl From<LoadError> for Phase5BootError {
    fn from(error: LoadError) -> Self {
        Self::Load(error)
    }
}

impl From<user::UserLaunchError> for Phase5BootError {
    fn from(error: user::UserLaunchError) -> Self {
        Self::UserLaunch(error)
    }
}

#[derive(Clone, Copy, Debug)]
struct Phase5Summary {
    user_task: u64,
    user_thread: ThreadId,
    user_root: u64,
    image: LoadedUserImage,
    switch_to_user: ScheduleDecision,
    switch_back_to_kernel: ScheduleDecision,
    exit_code: u64,
}

#[entry]
fn kernel_main() -> Status {
    cpu::disable_interrupts();
    serial::init();
    serial::write_line("serviceos: entered x86_64 UEFI kernel image");

    let boot_context = exit_boot_services_and_capture_context();
    let mut mapper = unsafe { ActivePageTable::new_identity_mapped() };
    let kernel = match Kernel::initialize(&boot_context, &mut mapper, TIMER_TICK_HZ as u64) {
        Ok(kernel) => kernel,
        Err(error) => {
            serial::write_args(format_args!("serviceos: kernel init failed: {error:?}\n"));
            cpu::halt_loop()
        }
    };
    let descriptor_state = interrupts::initialize();
    serial::write_args(format_args!(
        "serviceos: memory regions = {} (usable = {}, boot-services reclaimable = {})\n",
        kernel.boot_context().memory_region_count(),
        kernel.boot_context().usable_memory_region_count(),
        kernel
            .boot_context()
            .boot_services_reclaimable_region_count()
    ));
    serial::write_args(format_args!(
        "serviceos: root page table = {:#x}, heap = {:#x}..{:#x}, usable = {} MiB, remaining = {} MiB\n",
        kernel
            .memory()
            .kernel_address_space()
            .root
            .level_4_frame
            .as_u64(),
        kernel.memory().heap().range.start.as_u64(),
        kernel.memory().heap().range.end.as_u64(),
        kernel.memory().stats().usable_bytes / (1024 * 1024),
        kernel.memory().stats().remaining_usable_bytes / (1024 * 1024),
    ));
    serial::write_args(format_args!(
        "serviceos: idt={} gdt={} tss={} pic={} pit={} timer={}Hz syscall-vector={}\n",
        descriptor_state.idt_loaded,
        descriptor_state.gdt_loaded,
        descriptor_state.tss_loaded,
        descriptor_state.pic_remapped,
        descriptor_state.pit_programmed,
        descriptor_state.timer_hz,
        descriptor_state.syscall_vector.0,
    ));

    let summary = match launch_first_userspace(&kernel) {
        Ok(summary) => summary,
        Err(error) => {
            serial::write_args(format_args!(
                "serviceos: phase5 userspace launch failed: {error}\n"
            ));
            cpu::halt_loop()
        }
    };

    let scheduler_snapshot = kernel.tasks().snapshot().scheduler;
    let user_thread_state = kernel
        .objects()
        .registry()
        .lookup(ObjectId(summary.user_thread.0))
        .and_then(|object| object.thread().map(|thread| thread.snapshot()))
        .ok_or(Phase5BootError::MissingThreadObject)
        .unwrap_or_else(|error| {
            serial::write_args(format_args!("serviceos: phase5 snapshot failed: {error}\n"));
            cpu::halt_loop()
        });

    serial::write_args(format_args!(
        "serviceos: phase5 user-task={} user-thread={} user-root={:#x}\n",
        summary.user_task, summary.user_thread.0, summary.user_root,
    ));
    serial::write_args(format_args!(
        "serviceos: phase5 image entry={:#x} stack={:#x} code={} stack-bytes={}\n",
        summary.image.entry_point.as_u64(),
        summary.image.user_stack_top.as_u64(),
        summary.image.code_size,
        summary.image.mapped_stack_bytes,
    ));
    serial::write_args(format_args!(
        "serviceos: phase5 switches to-user={:?} to-kernel={:?} current={:?} runnable={} blocked={} switches={}\n",
        summary.switch_to_user.next.map(|thread| thread.0),
        summary.switch_back_to_kernel.next.map(|thread| thread.0),
        scheduler_snapshot.current.map(|thread| thread.0),
        scheduler_snapshot.runnable_threads,
        scheduler_snapshot.blocked_threads,
        scheduler_snapshot.context_switches,
    ));
    serial::write_args(format_args!(
        "serviceos: phase5 user-thread mode={:?} state={:?} wait={:?} wake={:?} exit-code={:#x} expected-base={:#x}\n",
        user_thread_state.mode,
        user_thread_state.execution_state,
        user_thread_state.wait_target,
        user_thread_state.last_wake_reason,
        summary.exit_code,
        userspace_demo::expected_exit_low32() as u64,
    ));
    serial::write_line("serviceos: phase5 userspace bootstrap initialized; halting");

    cpu::halt_loop()
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    serial::write_args(format_args!("serviceos: panic: {info}\n"));
    cpu::halt_loop()
}

fn launch_first_userspace(kernel: &Kernel<'_>) -> Result<Phase5Summary, Phase5BootError> {
    let objects = kernel.objects();
    let registry = objects.registry();
    let bootstrap_task = objects
        .bootstrap_task()
        .task()
        .ok_or(Phase5BootError::MissingTaskObject)?;
    let bootstrap_space = bootstrap_task.capability_space();
    let scheduler = kernel.tasks().scheduler();
    let bootstrap_thread = kernel.tasks().bootstrap_thread();

    let user_task = registry.create_task(TaskDescriptor {
        address_space: Some(AddressSpaceId(1)),
        role: TaskRole::SystemService,
    });
    let user_task_object = user_task.task().ok_or(Phase5BootError::MissingTaskObject)?;
    let user_task_id = user_task_object.id().0;
    let _user_task_handle = bootstrap_space.install(
        Arc::clone(&user_task),
        CapabilityRights::task(),
        Some(0x700),
    );

    let mut frame_allocator = kernel.memory().frame_allocator().lock();
    let mut user_page_table = unsafe {
        OwnedPageTable::new_user_space(
            kernel.memory().kernel_address_space().root.level_4_frame,
            &mut frame_allocator,
        )
    }?;
    let image = serviceos_kernel_core::user::load_flat_image(
        userspace_demo::image(),
        &mut user_page_table,
        &mut frame_allocator,
    )?;
    drop(frame_allocator);

    let user_thread = registry.create_thread(
        &user_task,
        ThreadDescriptor {
            mode: ThreadMode::User,
            scheduling_context: SchedulingContext::round_robin_default(),
            entry_instruction_pointer: Some(image.entry_point.as_u64()),
            stack_pointer: Some(image.user_stack_top.as_u64()),
        },
    );
    let user_thread_id = user_thread
        .thread()
        .ok_or(Phase5BootError::MissingThreadObject)?
        .id();
    scheduler.register_thread(user_thread)?;
    let _ = scheduler.make_runnable(
        user_thread_id,
        serviceos_kernel_core::task::ThreadWakeReason::Explicit,
    )?;

    let switch_to_user = scheduler.yield_current()?;
    ensure_next_thread(&switch_to_user, user_thread_id)?;
    serial::write_args(format_args!(
        "serviceos: phase5 entering user entry={:#x} stack={:#x} root={:#x}\n",
        image.entry_point.as_u64(),
        image.user_stack_top.as_u64(),
        user_page_table.root_frame().as_u64(),
    ));

    let exit_status = user::run_user_program(
        user_page_table.root_frame(),
        image.entry_point.as_u64(),
        image.user_stack_top.as_u64(),
    )?;
    serial::write_args(format_args!(
        "serviceos: phase5 returned from user with exit={:#x}\n",
        exit_status.code,
    ));

    let switch_back_to_kernel = scheduler.terminate_current()?;
    ensure_next_thread(&switch_back_to_kernel, bootstrap_thread)?;

    Ok(Phase5Summary {
        user_task: user_task_id,
        user_thread: user_thread_id,
        user_root: user_page_table.root_frame().as_u64(),
        image,
        switch_to_user,
        switch_back_to_kernel,
        exit_code: exit_status.code,
    })
}

fn ensure_next_thread(
    decision: &ScheduleDecision,
    expected: ThreadId,
) -> Result<(), Phase5BootError> {
    if decision.next == Some(expected) {
        Ok(())
    } else {
        Err(Phase5BootError::UnexpectedSchedule {
            expected,
            actual: decision.next,
        })
    }
}
