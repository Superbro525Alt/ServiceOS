#![no_main]
#![no_std]

extern crate alloc;

use alloc::sync::Arc;
use core::panic::PanicInfo;
use serviceos_kernel_arch_x86_64::{
    boot::exit_boot_services_and_capture_context,
    cpu,
    interrupts::{self, TIMER_TICK_HZ},
    paging::ActivePageTable,
    serial,
};
use serviceos_kernel_core::{
    Kernel,
    capability::{CapabilityError, CapabilityRights, TransferMode},
    ipc::{IpcError, MessageTag, OutgoingMessage},
    object::ObjectId,
    task::{
        ScheduleDecision, SchedulerError, SchedulingContext, TaskDescriptor, TaskRole,
        ThreadDescriptor, ThreadId, ThreadMode, ThreadWakeReason,
    },
    time::WakeEvent,
};
use uefi::{Status, entry};

enum Phase4DemoError {
    MissingTaskObject,
    MissingThreadObject,
    MissingTransferredCapability,
    UnexpectedSchedule {
        expected: ThreadId,
        actual: Option<ThreadId>,
    },
    UnexpectedWakeToken {
        expected: u64,
        actual: u64,
    },
    Capability(CapabilityError),
    Ipc(IpcError),
    Scheduler(SchedulerError),
}

impl core::fmt::Display for Phase4DemoError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::MissingTaskObject => formatter.write_str("missing task object"),
            Self::MissingThreadObject => formatter.write_str("missing thread object"),
            Self::MissingTransferredCapability => {
                formatter.write_str("missing transferred capability")
            }
            Self::UnexpectedSchedule { expected, actual } => write!(
                formatter,
                "unexpected schedule outcome: expected thread {} got {:?}",
                expected.0, actual
            ),
            Self::UnexpectedWakeToken { expected, actual } => write!(
                formatter,
                "unexpected wake token: expected {expected} got {actual}"
            ),
            Self::Capability(error) => write!(formatter, "capability error: {error:?}"),
            Self::Ipc(error) => write!(formatter, "ipc error: {error:?}"),
            Self::Scheduler(error) => write!(formatter, "scheduler error: {error:?}"),
        }
    }
}

impl From<CapabilityError> for Phase4DemoError {
    fn from(error: CapabilityError) -> Self {
        Self::Capability(error)
    }
}

impl From<IpcError> for Phase4DemoError {
    fn from(error: IpcError) -> Self {
        Self::Ipc(error)
    }
}

impl From<SchedulerError> for Phase4DemoError {
    fn from(error: SchedulerError) -> Self {
        Self::Scheduler(error)
    }
}

#[derive(Clone, Copy, Debug)]
struct Phase4PreparedDemo {
    service_task: u64,
    service_thread: ThreadId,
    transferred_object: u64,
    timer_token: u64,
    switch_to_service: ScheduleDecision,
    block_on_receive: ScheduleDecision,
    switch_after_ipc: ScheduleDecision,
    block_on_timer: ScheduleDecision,
}

#[derive(Clone, Copy, Debug)]
struct Phase4Completion {
    wake_decision: Option<ScheduleDecision>,
    final_switch: ScheduleDecision,
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

    let prepared = match prepare_phase4_demo(&kernel) {
        Ok(prepared) => prepared,
        Err(error) => {
            serial::write_args(format_args!("serviceos: phase4 setup failed: {error}\n"));
            cpu::halt_loop()
        }
    };
    serial::write_args(format_args!(
        "serviceos: phase4 service-task={} service-thread={} transferred-object={} timer-token={}\n",
        prepared.service_task,
        prepared.service_thread.0,
        prepared.transferred_object,
        prepared.timer_token,
    ));
    serial::write_args(format_args!(
        "serviceos: phase4 switches boot->svc={:?} recv-block={:?} ipc-resume={:?} timer-block={:?}\n",
        prepared.switch_to_service.next.map(|thread| thread.0),
        prepared.block_on_receive.next.map(|thread| thread.0),
        prepared.switch_after_ipc.next.map(|thread| thread.0),
        prepared.block_on_timer.next.map(|thread| thread.0),
    ));

    serial::write_line("serviceos: enabling interrupts");
    cpu::enable_interrupts();
    serial::write_line("serviceos: waiting for scheduled timer wakeup");

    loop {
        if let Some(event) = interrupts::poll_wakeup() {
            let completion = match complete_phase4_demo(&kernel, &prepared, event) {
                Ok(completion) => completion,
                Err(error) => {
                    serial::write_args(format_args!(
                        "serviceos: phase4 completion failed: {error}\n"
                    ));
                    cpu::halt_loop()
                }
            };
            let task_snapshot = kernel.tasks().snapshot();
            let service_thread_state = kernel
                .objects()
                .registry()
                .lookup(ObjectId(prepared.service_thread.0))
                .and_then(|object| object.thread().map(|thread| thread.snapshot()))
                .ok_or(Phase4DemoError::MissingThreadObject)
                .unwrap_or_else(|error| {
                    serial::write_args(format_args!(
                        "serviceos: phase4 snapshot failed: {error}\n"
                    ));
                    cpu::halt_loop()
                });
            let trap_stats = kernel.interrupts().snapshot();
            let syscall_stats = kernel.syscalls().snapshot();
            let time_snapshot = kernel.time().snapshot();

            serial::write_args(format_args!(
                "serviceos: wake token={} reason={:?} now={} ticks pending={} ready={}\n",
                event.token.0,
                event.reason,
                time_snapshot.now.0,
                time_snapshot.pending_timers,
                time_snapshot.ready_wakeups,
            ));
            serial::write_args(format_args!(
                "serviceos: phase4 wake-decision={:?} final-switch={:?} current={:?} runnable={} blocked={} switches={}\n",
                completion
                    .wake_decision
                    .and_then(|decision| decision.next.map(|thread| thread.0)),
                completion.final_switch.next.map(|thread| thread.0),
                task_snapshot.scheduler.current.map(|thread| thread.0),
                task_snapshot.scheduler.runnable_threads,
                task_snapshot.scheduler.blocked_threads,
                task_snapshot.scheduler.context_switches,
            ));
            serial::write_args(format_args!(
                "serviceos: phase4 thread-state mode={:?} state={:?} wait={:?} wake={:?}\n",
                service_thread_state.mode,
                service_thread_state.execution_state,
                service_thread_state.wait_target,
                service_thread_state.last_wake_reason,
            ));
            serial::write_args(format_args!(
                "serviceos: trap-stats exceptions={} external={} timer={} syscalls={} dispatcher={} rejected={}\n",
                trap_stats.exceptions,
                trap_stats.external_interrupts,
                trap_stats.timer_interrupts,
                trap_stats.syscalls,
                syscall_stats.dispatched,
                syscall_stats.rejected,
            ));
            serial::write_line(
                "serviceos: phase4 scheduler and process foundation initialized; halting",
            );
            break;
        }

        cpu::halt();
    }

    cpu::halt_loop()
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    serial::write_args(format_args!("serviceos: panic: {info}\n"));
    cpu::halt_loop()
}

fn prepare_phase4_demo(kernel: &Kernel<'_>) -> Result<Phase4PreparedDemo, Phase4DemoError> {
    let objects = kernel.objects();
    let registry = objects.registry();
    let bootstrap_task = objects
        .bootstrap_task()
        .task()
        .ok_or(Phase4DemoError::MissingTaskObject)?;
    let bootstrap_space = bootstrap_task.capability_space();
    let scheduler = kernel.tasks().scheduler();
    let bootstrap_thread = kernel.tasks().bootstrap_thread();

    let service_task = registry.create_task(TaskDescriptor {
        address_space: None,
        role: TaskRole::SystemService,
    });
    let service_task_object = service_task
        .task()
        .ok_or(Phase4DemoError::MissingTaskObject)?;
    let service_task_id = service_task_object.id().0;
    let service_space = service_task_object.capability_space();
    let _service_task_handle = bootstrap_space.install(
        Arc::clone(&service_task),
        CapabilityRights::task(),
        Some(0x600),
    );

    let service_thread = registry.create_thread(
        &service_task,
        ThreadDescriptor {
            mode: ThreadMode::Kernel,
            scheduling_context: SchedulingContext::round_robin_default(),
            entry_instruction_pointer: Some(0x0040_0000),
            stack_pointer: Some(0x0080_0000),
        },
    );
    let service_thread_id = service_thread
        .thread()
        .ok_or(Phase4DemoError::MissingThreadObject)?
        .id();
    scheduler.register_thread(service_thread)?;
    let _ = scheduler.make_runnable(service_thread_id, ThreadWakeReason::Explicit)?;

    let switch_to_service = scheduler.yield_current()?;
    ensure_next_thread(&switch_to_service, service_thread_id)?;

    let (bootstrap_endpoint, service_endpoint) = kernel.ipc().create_channel_pair(objects);
    let bootstrap_endpoint_handle = bootstrap_space.install(
        bootstrap_endpoint,
        CapabilityRights::channel_endpoint(),
        Some(0x100),
    );
    let service_endpoint_handle = service_space.install(
        service_endpoint,
        CapabilityRights::channel_endpoint(),
        Some(0x200),
    );

    let memory_object = registry.create_memory_object(16 * 1024, true);
    let bootstrap_memory_handle = bootstrap_space.install(
        memory_object,
        CapabilityRights::memory_object(),
        Some(0x300),
    );
    let transferred_memory = bootstrap_space.prepare_transfer(
        bootstrap_memory_handle,
        CapabilityRights::READ.union(CapabilityRights::MAP),
        TransferMode::Copy,
    )?;

    let service_receive_endpoint = kernel.ipc().endpoint_object_id(
        service_space,
        service_endpoint_handle,
        CapabilityRights::RECEIVE,
    )?;
    let block_on_receive = scheduler.block_current_on_receive(service_receive_endpoint)?;
    ensure_next_thread(&block_on_receive, bootstrap_thread)?;

    let request = OutgoingMessage::new(MessageTag(0x20), &[0xDEAD_BEEF, service_task_id])?
        .add_transfer(transferred_memory)?;
    let _ = kernel
        .ipc()
        .send(bootstrap_space, bootstrap_endpoint_handle, request)?;

    let switch_after_ipc = scheduler.yield_current()?;
    ensure_next_thread(&switch_after_ipc, service_thread_id)?;

    let received = kernel
        .ipc()
        .receive(service_space, service_endpoint_handle)?;
    let transferred_handle = *received
        .transferred_capabilities
        .first()
        .ok_or(Phase4DemoError::MissingTransferredCapability)?;
    let transferred_object = service_space
        .resolve(transferred_handle, CapabilityRights::READ)?
        .object
        .id()
        .0;

    let deadline = kernel.time().now().saturating_add(5);
    let (timer_token, block_on_timer) = scheduler.block_current_until(deadline)?;
    ensure_next_thread(&block_on_timer, bootstrap_thread)?;

    Ok(Phase4PreparedDemo {
        service_task: service_task_id,
        service_thread: service_thread_id,
        transferred_object,
        timer_token: timer_token.0,
        switch_to_service,
        block_on_receive,
        switch_after_ipc,
        block_on_timer,
    })
}

fn complete_phase4_demo(
    kernel: &Kernel<'_>,
    prepared: &Phase4PreparedDemo,
    event: WakeEvent,
) -> Result<Phase4Completion, Phase4DemoError> {
    if event.token.0 != prepared.timer_token {
        return Err(Phase4DemoError::UnexpectedWakeToken {
            expected: prepared.timer_token,
            actual: event.token.0,
        });
    }

    let wake_decision = kernel.tasks().handle_time_wakeup(event);
    let final_switch = kernel.tasks().scheduler().yield_current()?;
    ensure_next_thread(&final_switch, prepared.service_thread)?;

    Ok(Phase4Completion {
        wake_decision,
        final_switch,
    })
}

fn ensure_next_thread(
    decision: &ScheduleDecision,
    expected: ThreadId,
) -> Result<(), Phase4DemoError> {
    if decision.next == Some(expected) {
        Ok(())
    } else {
        Err(Phase4DemoError::UnexpectedSchedule {
            expected,
            actual: decision.next,
        })
    }
}
