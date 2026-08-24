#![no_main]
#![no_std]

use core::{
    fmt,
    fmt::Write,
    panic::PanicInfo,
    str,
    sync::atomic::{AtomicBool, Ordering},
};

use serviceos_abi::{BootstrapPlatform, ControlTag, ServiceImageId};
use serviceos_bundle::BootStore;
use serviceos_kernel_arch_aarch64::{
    cpu,
    gic::{self, GicConfig, GicInitError},
    mmu::{ActivePageTable, MmioRegion},
    timer as kernel_timer, traps, user,
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
use serviceos_platform_raspi5::{boot, dtb::InterruptControllerRegions, timer, uart};
use serviceos_userspace_catalog::BOOT_STORE_IMAGE;
use spin::Once;

const TIMER_TICK_HZ: u64 = 100;

static HARDWARE_TICKS: AtomicBool = AtomicBool::new(false);

#[unsafe(no_mangle)]
#[unsafe(link_section = ".boot_stack")]
static mut BOOT_STACK: [u8; 64 * 1024] = [0; 64 * 1024];

core::arch::global_asm!(
    ".section .text._start, \"ax\"",
    ".globl _start",
    ".type _start, %function",
    "_start:",
    "mrs x2, mpidr_el1",
    "tst x2, #0xff",
    "b.ne 1f",
    "mrs x2, CurrentEL",
    "lsr x2, x2, #2",
    "cmp x2, #1",
    "b.eq 4f",
    "cmp x2, #2",
    "b.ne 1f",
    "mov x4, #0x80000000",
    "msr hcr_el2, x4",
    "mrs x4, cnthctl_el2",
    "orr x4, x4, #0x3",
    "msr cnthctl_el2, x4",
    "dsb sy",
    "isb",
    "mov x4, #0x3c5",
    "msr spsr_el2, x4",
    "adr x4, 4f",
    "msr elr_el2, x4",
    "eret",
    "4:",
    "adrp x1, {stack}",
    "add x1, x1, :lo12:{stack}",
    "mov x3, {size}",
    "add x1, x1, x3",
    "mov sp, x1",
    "b {entry}",
    "1:",
    "wfe",
    "b 1b",
    size = const 64 * 1024,
    stack = sym BOOT_STACK,
    entry = sym serviceos_raspi5_entry,
);

unsafe extern "C" {
    static __image_start: u8;
    static __image_end: u8;
}

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

#[derive(Clone, Copy)]
struct TimerPollState {
    last_counter: u64,
    cycles_per_tick: u64,
    carry_cycles: u64,
}

#[unsafe(no_mangle)]
extern "C" fn serviceos_raspi5_entry(dtb_ptr: usize) -> ! {
    cpu::disable_interrupts();
    let kernel_start = PhysicalAddress::new(core::ptr::addr_of!(__image_start) as u64);
    let kernel_end = PhysicalAddress::new(core::ptr::addr_of!(__image_end) as u64);
    let mut boot_state =
        match boot::capture_boot_info(dtb_ptr as *const u8, kernel_start, kernel_end) {
            Ok(state) => state,
            Err(error) => panic_with_error("boot", error),
        };

    if let Some(descriptor) = boot_state.summary.uart {
        uart::initialize(descriptor);
    }
    boot_state.boot_info.boot_store = Some(BOOT_STORE_IMAGE);

    log_line("boot", "entered Raspberry Pi 5 kernel image");
    log(
        "boot",
        format_args!(
            "model={} compatible={} serial={} current-el={} core={}",
            boot_state.summary.model,
            boot_state.summary.compatible.unwrap_or("unknown"),
            boot_state.summary.serial_number.unwrap_or("unknown"),
            cpu::current_el(),
            cpu::core_id(),
        ),
    );
    log(
        "boot",
        format_args!(
            "dtb-base={:#x} dtb-bytes={} boot-store-bytes={}",
            boot_state.summary.dtb_base.as_u64(),
            boot_state.summary.dtb_size,
            BOOT_STORE_IMAGE.len(),
        ),
    );
    if let Some(uart) = boot_state.summary.uart {
        log(
            "serial",
            format_args!(
                "stdout-path={} base={:#x} span={} compatible={}",
                uart.path,
                uart.base.as_u64(),
                uart.span,
                uart.compatible.unwrap_or("unknown"),
            ),
        );
    }

    let mut mmio_regions = [MmioRegion {
        base: PhysicalAddress::new(0),
        size: 0,
    }; 3];
    let mut mmio_region_count = 0usize;
    if let Some(descriptor) = boot_state.summary.uart {
        mmio_regions[mmio_region_count] = MmioRegion {
            base: descriptor.base,
            size: descriptor.span,
        };
        mmio_region_count += 1;
    }
    if let Some(controller) = boot_state.summary.interrupt_controller {
        for range in [controller.distributor, controller.redistributors] {
            mmio_regions[mmio_region_count] = MmioRegion {
                base: range.start,
                size: range.span_bytes() as usize,
            };
            mmio_region_count += 1;
        }
    }

    let mut mapper = match ActivePageTable::initialize(
        &boot_state.boot_info,
        &mmio_regions[..mmio_region_count],
    ) {
        Ok(mapper) => mapper,
        Err(error) => panic_with_error("memory", error),
    };

    let kernel = match Kernel::initialize(&boot_state.boot_info, &mut mapper, TIMER_TICK_HZ) {
        Ok(kernel) => kernel,
        Err(error) => panic_with_error("boot", error),
    };
    traps::initialize();
    user::initialize();
    kernel_user::initialize_runtime();
    let _ = BOOT_STORE_IMAGE_SOURCE.call_once(|| BOOT_STORE_IMAGE);
    kernel_user::register_image_resolver(resolve_boot_store_image);
    syscall::register_debug_log_writer(debug_log_writer);
    syscall::register_debug_console_reader(uart::try_read_byte);
    syscall::register_debug_console_writer(uart::write_bytes);

    log_memory_summary(&boot_state.boot_info);
    log(
        "timer",
        format_args!(
            "backend=arm-generic freq={} tick-hz={}",
            timer::counter_frequency_hz(),
            TIMER_TICK_HZ,
        ),
    );
    match bring_up_interrupts(boot_state.summary.interrupt_controller) {
        Ok(interval_cycles) => {
            HARDWARE_TICKS.store(true, Ordering::Relaxed);
            log(
                "interrupts",
                format_args!(
                    "backend=gic-v3 timer=el1-physical ppi={} tick-hz={} interval-cycles={}",
                    gic::TIMER_PPI_INTID,
                    TIMER_TICK_HZ,
                    interval_cycles,
                ),
            );
        }
        Err(reason) => {
            log(
                "interrupts",
                format_args!("backend=polling reason={reason}"),
            );
        }
    }
    log_line("bootstrap", "starting serial-first userspace graph");

    let summary = match launch_root_manager(&kernel) {
        Ok(summary) => summary,
        Err(error) => panic_with_error("bootstrap", error),
    };

    let root_thread_state = kernel
        .objects()
        .registry()
        .lookup(ObjectId(summary.root_thread.0))
        .and_then(|object| object.thread().map(|thread| thread.snapshot()))
        .ok_or(BootstrapError::MissingRootThread)
        .unwrap_or_else(|error| panic_with_error("bootstrap", error));

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
    cpu::wait_forever()
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    log("panic", format_args!("{info}"));
    cpu::wait_forever()
}

use serviceos_kernel_core::memory::PhysicalAddress;

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
        &[
            boot_store_bytes.len() as u64,
            BootstrapPlatform::Raspi5 as u32 as u64,
            0,
            0,
        ],
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

fn bring_up_interrupts(
    controller: Option<InterruptControllerRegions>,
) -> Result<u64, &'static str> {
    let Some(controller) = controller else {
        return Err("device-tree-gic-missing");
    };
    let config = GicConfig {
        distributor_base: controller.distributor.start,
        redistributor_base: controller.redistributors.start,
    };
    gic::initialize(config).map_err(|error| match error {
        GicInitError::Unavailable => "gic-unavailable",
        GicInitError::MisalignedRegion => "gic-region-alignment",
        GicInitError::RedistributorWakeTimeout => "gic-redistributor-wake-timeout",
        GicInitError::DistributorWriteTimeout => "gic-distributor-write-timeout",
        GicInitError::SystemRegisterUnsupported => "gic-system-register-unsupported",
    })?;
    kernel_timer::arm_periodic_tick(TIMER_TICK_HZ)
        .map_err(|_| "timer-counter-frequency-unavailable")
}

fn run_userspace_executor(
    kernel: &Kernel<'_>,
    root_task: serviceos_kernel_core::task::TaskId,
) -> Result<(), BootstrapError> {
    let hardware_ticks = HARDWARE_TICKS.load(Ordering::Relaxed);
    let mut timer_state = initialize_timer_poll_state();
    loop {
        if hardware_ticks {
            while let Some(event) = kernel.time().take_wakeup() {
                let _ = kernel.tasks().handle_time_wakeup(event);
            }
        } else {
            poll_timer_wakeups(kernel, &mut timer_state);
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
            if hardware_ticks && snapshot.blocked_threads > 0 {
                cpu::enable_irqs();
                cpu::wait_for_interrupt();
                cpu::disable_irqs();
                continue;
            }
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

        if hardware_ticks && (kernel.tasks().consume_preemption() || snapshot.preemption_pending) {
            let _ = scheduler.preempt_current_if_needed()?;
            continue;
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

fn initialize_timer_poll_state() -> TimerPollState {
    let frequency = timer::counter_frequency_hz().max(1);
    TimerPollState {
        last_counter: timer::counter_value(),
        cycles_per_tick: (frequency / TIMER_TICK_HZ.max(1)).max(1),
        carry_cycles: 0,
    }
}

fn poll_timer_wakeups(kernel: &Kernel<'_>, state: &mut TimerPollState) {
    let now = timer::counter_value();
    let elapsed = now.saturating_sub(state.last_counter);
    state.last_counter = now;
    state.carry_cycles = state.carry_cycles.saturating_add(elapsed);
    while state.carry_cycles >= state.cycles_per_tick {
        state.carry_cycles -= state.cycles_per_tick;
        let _ = kernel.time().handle_tick();
    }
    while let Some(event) = kernel.time().take_wakeup() {
        let _ = kernel.tasks().handle_time_wakeup(event);
    }
}

fn resolve_boot_store_image(image_id: u32) -> Option<&'static [u8]> {
    let boot_store = BOOT_STORE_IMAGE_SOURCE.get().copied()?;
    BootStore::parse(boot_store).ok()?.resolve_image(image_id)
}

fn debug_log_writer(bytes: &[u8]) {
    if let Ok(text) = str::from_utf8(bytes) {
        log("service", format_args!("{text}"));
    } else {
        log("service", format_args!("<non-utf8 {} bytes>", bytes.len()));
    }
}

fn log_memory_summary(boot_info: &serviceos_kernel_core::bootstrap::BootInfo<'_>) {
    log(
        "memory",
        format_args!(
            "regions={} usable={} boot-services-reclaimable={} boot-store={}",
            boot_info.memory_region_count(),
            boot_info.usable_memory_region_count(),
            boot_info.boot_services_reclaimable_region_count(),
            if boot_info.boot_store.is_some() {
                "present"
            } else {
                "missing"
            },
        ),
    );
}

fn panic_with_error(scope: &str, error: impl fmt::Debug) -> ! {
    log(scope, format_args!("bring-up failed: {error:?}"));
    cpu::wait_forever()
}

fn log_line(scope: &str, message: &str) {
    log(scope, format_args!("{message}"));
}

fn log(scope: &str, message: fmt::Arguments<'_>) {
    uart::write_bytes(b"serviceos: ");
    uart::write_bytes(scope.as_bytes());
    uart::write_bytes(b": ");
    let _ = UartLogWriter.write_fmt(message);
    uart::write_bytes(b"\r\n");
}

struct UartLogWriter;

impl fmt::Write for UartLogWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        uart::write_bytes(s.as_bytes());
        Ok(())
    }
}
