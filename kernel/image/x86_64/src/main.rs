#![no_main]
#![no_std]

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
    task::{ExecutionState, TaskDescriptor, TaskRole, ThreadDescriptor},
};
use uefi::{Status, entry};

enum Phase3SelfCheckError {
    MissingTaskObject,
    MissingThreadObject,
    MissingTransferredCapability,
    UnexpectedTransferredRights,
    UnexpectedAck,
    Capability(CapabilityError),
    Ipc(IpcError),
}

impl core::fmt::Display for Phase3SelfCheckError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::MissingTaskObject => formatter.write_str("missing task object"),
            Self::MissingThreadObject => formatter.write_str("missing thread object"),
            Self::MissingTransferredCapability => {
                formatter.write_str("missing transferred capability")
            }
            Self::UnexpectedTransferredRights => {
                formatter.write_str("unexpected transferred rights")
            }
            Self::UnexpectedAck => formatter.write_str("unexpected ack payload"),
            Self::Capability(error) => write!(formatter, "capability error: {error:?}"),
            Self::Ipc(error) => write!(formatter, "ipc error: {error:?}"),
        }
    }
}

impl From<CapabilityError> for Phase3SelfCheckError {
    fn from(error: CapabilityError) -> Self {
        Self::Capability(error)
    }
}

impl From<IpcError> for Phase3SelfCheckError {
    fn from(error: IpcError) -> Self {
        Self::Ipc(error)
    }
}

#[derive(Clone, Copy, Debug)]
struct Phase3SelfCheckSummary {
    tracked_objects_after_cleanup: usize,
    bootstrap_handle_count: usize,
    service_handle_count_before_cleanup: usize,
    transferred_object: u64,
    transferred_rights: u64,
    service_thread_state: ExecutionState,
    timer_armed: bool,
    event_signal_count: u64,
    ack_word: u64,
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
    match run_phase3_self_check(&kernel) {
        Ok(summary) => {
            serial::write_args(format_args!(
                "serviceos: phase3 objects tracked={} bootstrap-handles={} service-handles={} transferred-object={} transferred-rights={:#x}\n",
                summary.tracked_objects_after_cleanup,
                summary.bootstrap_handle_count,
                summary.service_handle_count_before_cleanup,
                summary.transferred_object,
                summary.transferred_rights,
            ));
            serial::write_args(format_args!(
                "serviceos: phase3 thread-state={:?} timer-armed={} event-signals={} ack-word={:#x}\n",
                summary.service_thread_state,
                summary.timer_armed,
                summary.event_signal_count,
                summary.ack_word,
            ));
        }
        Err(error) => {
            serial::write_args(format_args!(
                "serviceos: phase3 self-check failed: {error}\n"
            ));
            cpu::halt_loop()
        }
    }

    serial::write_line("serviceos: arming demo wakeup");
    interrupts::arm_demo_wakeup(5);
    serial::write_line("serviceos: enabling interrupts");
    cpu::enable_interrupts();
    serial::write_line("serviceos: waiting for wakeup");

    loop {
        if let Some(event) = interrupts::poll_wakeup() {
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
                "serviceos: trap-stats exceptions={} external={} timer={} syscalls={} dispatcher={} rejected={}\n",
                trap_stats.exceptions,
                trap_stats.external_interrupts,
                trap_stats.timer_interrupts,
                trap_stats.syscalls,
                syscall_stats.dispatched,
                syscall_stats.rejected,
            ));
            serial::write_line("serviceos: phase3 object and IPC foundation initialized; halting");
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

fn run_phase3_self_check(
    kernel: &Kernel<'_>,
) -> Result<Phase3SelfCheckSummary, Phase3SelfCheckError> {
    let objects = kernel.objects();
    let registry = objects.registry();
    let bootstrap_task = objects
        .bootstrap_task()
        .task()
        .ok_or(Phase3SelfCheckError::MissingTaskObject)?;
    let bootstrap_space = bootstrap_task.capability_space();

    let service_task = registry.create_task(TaskDescriptor {
        address_space: None,
        role: TaskRole::SystemService,
    });
    let service_task_object = service_task
        .task()
        .ok_or(Phase3SelfCheckError::MissingTaskObject)?;
    let service_space = service_task_object.capability_space();

    let service_thread = registry.create_thread(
        &service_task,
        ThreadDescriptor {
            entry_instruction_pointer: Some(0x0040_0000),
            stack_pointer: Some(0x0080_0000),
        },
    );
    let service_thread_object = service_thread
        .thread()
        .ok_or(Phase3SelfCheckError::MissingThreadObject)?;
    service_thread_object.set_execution_state(ExecutionState::Runnable, None);
    let service_thread_state = service_thread_object.snapshot().execution_state;

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

    let timer_object = registry.create_timer(Some(kernel.time().now().saturating_add(25)), None);
    let timer_armed = timer_object.timer().expect("timer object").snapshot().armed;
    let service_timer_handle =
        service_space.install(timer_object, CapabilityRights::timer(), Some(0x400));

    let event_object = registry.create_event(false);
    event_object.event().expect("event object").signal();
    let event_signal_count = event_object
        .event()
        .expect("event object")
        .snapshot()
        .signal_count;
    let service_event_handle =
        service_space.install(event_object, CapabilityRights::event(), Some(0x500));

    let request = OutgoingMessage::new(MessageTag(0x10), &[0xCAFE_BABE, 2])?
        .add_transfer(transferred_memory)?;
    let receipt = kernel
        .ipc()
        .send(bootstrap_space, bootstrap_endpoint_handle, request)?;
    let received = kernel
        .ipc()
        .receive(service_space, service_endpoint_handle)?;
    let transferred_handle = *received
        .transferred_capabilities
        .first()
        .ok_or(Phase3SelfCheckError::MissingTransferredCapability)?;
    let transferred_view = service_space.resolve(transferred_handle, CapabilityRights::READ)?;
    if transferred_view.rights.contains(CapabilityRights::WRITE) {
        return Err(Phase3SelfCheckError::UnexpectedTransferredRights);
    }
    let transferred_object = transferred_view.object.id().0;
    let transferred_rights = transferred_view.rights.bits();

    let ack = OutgoingMessage::new(MessageTag(0x11), &[receipt.peer.0, transferred_object])?;
    kernel
        .ipc()
        .send(service_space, service_endpoint_handle, ack)?;
    let reply = kernel
        .ipc()
        .receive(bootstrap_space, bootstrap_endpoint_handle)?;
    let ack_word = *reply
        .words
        .get(1)
        .ok_or(Phase3SelfCheckError::UnexpectedAck)?;
    if ack_word != transferred_object {
        return Err(Phase3SelfCheckError::UnexpectedAck);
    }

    let service_handle_count_before_cleanup = service_space.handle_count();
    service_space.close(transferred_handle)?;
    service_space.close(service_event_handle)?;
    service_space.close(service_timer_handle)?;
    service_space.close(service_endpoint_handle)?;
    bootstrap_space.close(bootstrap_memory_handle)?;
    bootstrap_space.close(bootstrap_endpoint_handle)?;
    drop(transferred_view);

    drop(service_thread);
    drop(service_task);
    registry.collect_garbage();

    Ok(Phase3SelfCheckSummary {
        tracked_objects_after_cleanup: registry.snapshot().tracked_objects,
        bootstrap_handle_count: bootstrap_space.handle_count(),
        service_handle_count_before_cleanup,
        transferred_object,
        transferred_rights,
        service_thread_state,
        timer_armed,
        event_signal_count,
        ack_word,
    })
}
