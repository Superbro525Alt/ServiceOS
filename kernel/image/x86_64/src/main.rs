#![no_main]
#![no_std]

use core::{fmt, panic::PanicInfo, str};

use serviceos_abi::{ControlTag, ServiceImageId};
use serviceos_bundle::BootStore;
use serviceos_kernel_arch_x86_64::{
    boot::exit_boot_services_and_capture_context,
    cpu,
    interrupts::{self, TIMER_TICK_HZ},
    paging::ActivePageTable,
    serial, user,
};
use serviceos_kernel_core::{
    Kernel,
    capability::{CapabilityError, CapabilityRights, TransferMode},
    ipc::{self, IpcError, MessageTag, OutgoingMessage},
    object::ObjectId,
    syscall,
    task::{ExecutionState, SchedulerError, TaskRole, ThreadId, ThreadMode},
    user::{self as kernel_user, SpawnError, TaskExitStatus},
};
use spin::Once;
use uefi::{Status, entry};

static BOOT_STORE_IMAGE_SOURCE: Once<&'static [u8]> = Once::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BootstrapError {
    RootSpawn(SpawnError),
    Scheduler(SchedulerError),
    Capability(CapabilityError),
    Ipc(IpcError),
    MissingRootTask,
    MissingRootThread,
    MissingBootStore,
    UserRun(user::UserLaunchError),
}

impl From<SpawnError> for BootstrapError {
    fn from(error: SpawnError) -> Self {
        Self::RootSpawn(error)
    }
}

impl From<SchedulerError> for BootstrapError {
    fn from(error: SchedulerError) -> Self {
        Self::Scheduler(error)
    }
}

impl From<CapabilityError> for BootstrapError {
    fn from(error: CapabilityError) -> Self {
        Self::Capability(error)
    }
}

impl From<IpcError> for BootstrapError {
    fn from(error: IpcError) -> Self {
        Self::Ipc(error)
    }
}

impl From<user::UserLaunchError> for BootstrapError {
    fn from(error: user::UserLaunchError) -> Self {
        Self::UserRun(error)
    }
}

#[derive(Clone, Copy, Debug)]
struct RootBootstrapSummary {
    root_task: u64,
    root_thread: ThreadId,
    exit_status: TaskExitStatus,
    scheduler_current: Option<ThreadId>,
    runnable_threads: usize,
    blocked_threads: usize,
    context_switches: u64,
}

#[entry]
fn kernel_main() -> Status {
    cpu::disable_interrupts();
    serial::init();
    log_line("boot", "entered x86_64 UEFI kernel image");

    let boot_context = exit_boot_services_and_capture_context();
    let Some(boot_store) = boot_context.boot_store else {
        log_line("boot", "boot-store payload missing");
        cpu::halt_loop()
    };
    let mut mapper = unsafe { ActivePageTable::new_identity_mapped() };
    let kernel = match Kernel::initialize(&boot_context, &mut mapper, TIMER_TICK_HZ as u64) {
        Ok(kernel) => kernel,
        Err(error) => {
            log("boot", format_args!("kernel init failed: {error:?}"));
            cpu::halt_loop()
        }
    };
    let descriptor_state = interrupts::initialize();
    user::initialize();
    kernel_user::initialize_runtime();
    let _ = BOOT_STORE_IMAGE_SOURCE.call_once(|| boot_store);
    kernel_user::register_image_resolver(resolve_boot_store_image);
    syscall::register_debug_log_writer(debug_log_writer);
    syscall::register_debug_console_reader(serial::try_read_byte);
    syscall::register_debug_console_writer(serial::write_bytes);

    log(
        "memory",
        format_args!(
            "regions={} usable={} boot-services-reclaimable={} boot-store-bytes={}",
            kernel.boot_context().memory_region_count(),
            kernel.boot_context().usable_memory_region_count(),
            kernel
                .boot_context()
                .boot_services_reclaimable_region_count(),
            boot_store.len(),
        ),
    );
    log(
        "memory",
        format_args!(
            "root-page-table={:#x} heap={:#x}..{:#x} usable-mib={} remaining-mib={}",
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
        ),
    );
    log(
        "interrupt",
        format_args!(
            "idt={} gdt={} tss={} pic={} pit={} timer-hz={} syscall-vector={}",
            descriptor_state.idt_loaded,
            descriptor_state.gdt_loaded,
            descriptor_state.tss_loaded,
            descriptor_state.pic_remapped,
            descriptor_state.pit_programmed,
            descriptor_state.timer_hz,
            descriptor_state.syscall_vector.0,
        ),
    );

    let summary = match launch_root_manager(&kernel) {
        Ok(summary) => summary,
        Err(error) => {
            log(
                "bootstrap",
                format_args!("root userspace bootstrap failed: {error:?}"),
            );
            cpu::halt_loop()
        }
    };

    let root_thread_state = kernel
        .objects()
        .registry()
        .lookup(ObjectId(summary.root_thread.0))
        .and_then(|object| object.thread().map(|thread| thread.snapshot()))
        .ok_or(BootstrapError::MissingRootThread)
        .unwrap_or_else(|error| {
            log("bootstrap", format_args!("root snapshot failed: {error:?}"));
            cpu::halt_loop()
        });

    log(
        "process",
        format_args!(
            "root-task={} root-thread={} exit-status={:?}",
            summary.root_task, summary.root_thread.0, summary.exit_status,
        ),
    );
    log(
        "scheduler",
        format_args!(
            "current={:?} runnable={} blocked={} switches={}",
            summary.scheduler_current.map(|thread| thread.0),
            summary.runnable_threads,
            summary.blocked_threads,
            summary.context_switches,
        ),
    );
    log(
        "userspace",
        format_args!(
            "root-thread mode={:?} state={:?} wait={:?} wake={:?}",
            root_thread_state.mode,
            root_thread_state.execution_state,
            root_thread_state.wait_target,
            root_thread_state.last_wake_reason,
        ),
    );
    log_line(
        "bootstrap",
        "root userspace service graph completed; halting",
    );

    cpu::halt_loop()
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    log("panic", format_args!("{info}"));
    cpu::halt_loop()
}

fn launch_root_manager(kernel: &Kernel<'_>) -> Result<RootBootstrapSummary, BootstrapError> {
    let ipc_kernel = ipc::kernel().ok_or(BootstrapError::MissingBootStore)?;
    let bootstrap_task = kernel
        .objects()
        .bootstrap_task()
        .task()
        .ok_or(BootstrapError::MissingRootTask)?;
    let (kernel_bootstrap_endpoint, root_bootstrap_endpoint) =
        ipc_kernel.create_channel_pair(kernel.objects());
    let kernel_bootstrap_handle = bootstrap_task.capability_space().install(
        kernel_bootstrap_endpoint,
        CapabilityRights::channel_endpoint(),
        None,
    )?;
    let root_bootstrap_handle = bootstrap_task.capability_space().install(
        root_bootstrap_endpoint,
        CapabilityRights::channel_endpoint(),
        None,
    )?;
    let root_bootstrap_transfer = bootstrap_task.capability_space().prepare_transfer(
        root_bootstrap_handle,
        CapabilityRights::channel_endpoint(),
        TransferMode::Move,
    )?;
    let boot_store_bytes = kernel
        .boot_context()
        .boot_store
        .ok_or(BootstrapError::MissingBootStore)?;
    let boot_store_object = kernel
        .objects()
        .registry()
        .create_memory_object_from_bytes(boot_store_bytes);
    let boot_store_handle = bootstrap_task.capability_space().install(
        boot_store_object,
        CapabilityRights::READ
            .union(CapabilityRights::DUPLICATE)
            .union(CapabilityRights::TRANSFER),
        None,
    )?;
    let bootstrap_authority_handle = bootstrap_task.capability_space().install(
        kernel.objects().bootstrap_capability().clone(),
        CapabilityRights::bootstrap().union(CapabilityRights::TRANSFER),
        None,
    )?;
    let boot_store_transfer = bootstrap_task.capability_space().prepare_transfer(
        boot_store_handle,
        CapabilityRights::READ
            .union(CapabilityRights::DUPLICATE)
            .union(CapabilityRights::TRANSFER),
        TransferMode::Copy,
    )?;
    let bootstrap_authority_transfer = bootstrap_task.capability_space().prepare_transfer(
        bootstrap_authority_handle,
        CapabilityRights::bootstrap(),
        TransferMode::Move,
    )?;

    let root = kernel_user::spawn_builtin_task(
        ServiceImageId::RootManager as u32,
        TaskRole::SystemService,
        Some(root_bootstrap_transfer),
    )?;
    let startup = OutgoingMessage::new(
        MessageTag(ControlTag::Startup as u32),
        &[boot_store_bytes.len() as u64],
    )?
    .add_transfer(boot_store_transfer)?
    .add_transfer(bootstrap_authority_transfer)?;
    ipc_kernel.send(
        bootstrap_task.capability_space(),
        kernel_bootstrap_handle,
        startup,
    )?;
    let root_task = root
        .task
        .task()
        .ok_or(BootstrapError::MissingRootTask)?
        .id();
    let root_thread = root
        .thread
        .thread()
        .ok_or(BootstrapError::MissingRootThread)?
        .id();

    let _ = kernel.tasks().scheduler().yield_current()?;
    run_userspace_executor(kernel, root_task)?;

    let scheduler_snapshot = kernel.tasks().snapshot().scheduler;
    let exit_status = kernel_user::runtime()
        .and_then(|runtime| runtime.task_exit_status(root_task))
        .unwrap_or(TaskExitStatus::Running);

    Ok(RootBootstrapSummary {
        root_task: root_task.0,
        root_thread,
        exit_status,
        scheduler_current: scheduler_snapshot.current,
        runnable_threads: scheduler_snapshot.runnable_threads,
        blocked_threads: scheduler_snapshot.blocked_threads,
        context_switches: scheduler_snapshot.context_switches,
    })
}

fn resolve_boot_store_image(image_id: u32) -> Option<&'static [u8]> {
    let boot_store = BOOT_STORE_IMAGE_SOURCE.get().copied()?;
    BootStore::parse(boot_store).ok()?.resolve_image(image_id)
}

fn run_userspace_executor(
    kernel: &Kernel<'_>,
    root_task: serviceos_kernel_core::task::TaskId,
) -> Result<(), BootstrapError> {
    loop {
        while let Some(event) = interrupts::poll_wakeup() {
            let _ = kernel.tasks().handle_time_wakeup(event);
        }

        let scheduler = kernel.tasks().scheduler();
        let snapshot = scheduler.snapshot();
        let current = snapshot.current;
        let root_status = kernel_user::runtime()
            .and_then(|runtime| runtime.task_exit_status(root_task))
            .unwrap_or(TaskExitStatus::Running);

        if matches!(root_status, TaskExitStatus::Exited { .. }) && snapshot.runnable_threads == 0 {
            return Ok(());
        }

        let Some(thread_id) = current else {
            return Ok(());
        };

        if thread_id == kernel.tasks().bootstrap_thread() {
            if snapshot.runnable_threads > 0 {
                let _ = scheduler.yield_current()?;
                continue;
            }
            if snapshot.blocked_threads > 0 {
                log_line(
                    "bootstrap",
                    "userspace executor stalled with only blocked threads",
                );
                return Ok(());
            }
            return Ok(());
        }

        let Some(thread_object) = kernel.objects().registry().lookup(ObjectId(thread_id.0)) else {
            return Err(BootstrapError::MissingRootThread);
        };
        let Some(thread_state) = thread_object.thread().map(|thread| thread.snapshot()) else {
            return Err(BootstrapError::MissingRootThread);
        };

        if thread_state.mode == ThreadMode::User
            && thread_state.execution_state == ExecutionState::Running
        {
            user::run_thread(thread_id)?;
        } else {
            let _ = scheduler.yield_current()?;
        }
    }
}

fn debug_log_writer(bytes: &[u8]) {
    if let Ok(text) = str::from_utf8(bytes) {
        log("service", format_args!("{text}"));
    } else {
        log("service", format_args!("<non-utf8 {} bytes>", bytes.len()));
    }
}

fn log_line(domain: &str, message: &str) {
    serial::write_args(format_args!("serviceos: {domain}: {message}\n"));
}

fn log(domain: &str, args: fmt::Arguments<'_>) {
    serial::write_args(format_args!("serviceos: {domain}: {args}\n"));
}
