#![no_main]
#![no_std]

use core::{fmt, panic::PanicInfo, str};

use serviceos_abi::{ControlTag, ServiceImageId, bootstrap_resource};
use serviceos_bundle::BootStore;
use serviceos_kernel_arch_x86_64::{
    cpu,
    interrupts::{self, TIMER_TICK_HZ},
    paging::ActivePageTable,
    user,
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
use serviceos_platform_qemu_virtio::{audio, boot, display, input, network, serial};
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

    let boot_info = boot::capture_boot_info();
    let Some(boot_store) = boot_info.boot_store else {
        log_line("boot", "boot-store payload missing");
        cpu::halt_loop()
    };
    let mut mapper = unsafe { ActivePageTable::new_identity_mapped() };
    let kernel = match Kernel::initialize(&boot_info, &mut mapper, TIMER_TICK_HZ as u64) {
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

    let bootstrap_network = network::initialize()
        .map(|backend| kernel.objects().registry().create_packet_interface(backend));
    let bootstrap_display = kernel.boot_context().framebuffer.map(|framebuffer| {
        kernel
            .objects()
            .registry()
            .create_display_output(display::initialize(framebuffer))
    });
    let bootstrap_input =
        input::initialize().map(|backend| kernel.objects().registry().create_input_source(backend));
    let bootstrap_audio = Some(
        kernel
            .objects()
            .registry()
            .create_audio_endpoint(audio::initialize()),
    );

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
            "root-page-table={:#x} heap={:#x}..{:#x} usable-mib={} reclaimed-boot-mib={} remaining-mib={}",
            kernel
                .memory()
                .kernel_address_space()
                .root
                .level_4_frame
                .as_u64(),
            kernel.memory().heap().range.start.as_u64(),
            kernel.memory().heap().range.end.as_u64(),
            kernel.memory().stats().usable_bytes / (1024 * 1024),
            kernel.memory().stats().reclaimed_boot_services_bytes / (1024 * 1024),
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
    if let Some(summary) = network::bringup_summary() {
        log(
            "network",
            format_args!(
                "backend={:?} pci={:02x}:{:02x}.{} irq={} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} mtu={}",
                summary.backend,
                summary.pci_bus,
                summary.pci_device,
                summary.pci_function,
                summary.interrupt_line,
                summary.mac[0],
                summary.mac[1],
                summary.mac[2],
                summary.mac[3],
                summary.mac[4],
                summary.mac[5],
                summary.mtu,
            ),
        );
    } else {
        log_line("network", "no packet interface detected");
    }
    if let Some(framebuffer) = kernel.boot_context().framebuffer {
        log(
            "display",
            format_args!(
                "backend=BootFramebuffer {}x{} stride={} bpp={} bytes={} format={:?}",
                framebuffer.width,
                framebuffer.height,
                framebuffer.stride,
                framebuffer.bytes_per_pixel,
                framebuffer.byte_len,
                framebuffer.pixel_format,
            ),
        );
    } else {
        log_line("display", "no boot framebuffer detected");
    }
    if let Some(summary) = input::bringup_summary() {
        log(
            "input",
            format_args!(
                "backend={:?} keyboards={} pointers={}",
                summary.backend, summary.keyboard_devices, summary.pointer_devices,
            ),
        );
    } else {
        log_line("input", "no input source detected");
    }
    if let Some(summary) = audio::bringup_summary() {
        log(
            "audio",
            format_args!(
                "backend={:?} default-frequency-hz={}",
                summary.backend, summary.default_frequency_hz,
            ),
        );
    } else {
        log_line("audio", "no audio endpoint detected");
    }

    let summary = match launch_root_manager(
        &kernel,
        bootstrap_network,
        bootstrap_display,
        bootstrap_input,
        bootstrap_audio,
    ) {
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

fn launch_root_manager(
    kernel: &Kernel<'_>,
    bootstrap_network: Option<serviceos_kernel_core::object::KernelObjectRef>,
    bootstrap_display: Option<serviceos_kernel_core::object::KernelObjectRef>,
    bootstrap_input: Option<serviceos_kernel_core::object::KernelObjectRef>,
    bootstrap_audio: Option<serviceos_kernel_core::object::KernelObjectRef>,
) -> Result<RootBootstrapSummary, BootstrapError> {
    log_line("bootstrap", "preparing root-manager bootstrap channel");
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
    log_line(
        "bootstrap",
        "creating root-manager boot-store and authority transfers",
    );
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
    let network_transfer = if let Some(network_object) = bootstrap_network {
        let network_handle = bootstrap_task.capability_space().install(
            network_object,
            CapabilityRights::packet_interface(),
            None,
        )?;
        Some(bootstrap_task.capability_space().prepare_transfer(
            network_handle,
            CapabilityRights::packet_interface(),
            TransferMode::Move,
        )?)
    } else {
        None
    };
    let display_transfer = if let Some(display_object) = bootstrap_display {
        let display_handle = bootstrap_task.capability_space().install(
            display_object,
            CapabilityRights::display_output(),
            None,
        )?;
        Some(bootstrap_task.capability_space().prepare_transfer(
            display_handle,
            CapabilityRights::display_output(),
            TransferMode::Move,
        )?)
    } else {
        None
    };
    let input_transfer = if let Some(input_object) = bootstrap_input {
        let input_handle = bootstrap_task.capability_space().install(
            input_object,
            CapabilityRights::input_source(),
            None,
        )?;
        Some(bootstrap_task.capability_space().prepare_transfer(
            input_handle,
            CapabilityRights::input_source(),
            TransferMode::Move,
        )?)
    } else {
        None
    };
    let audio_transfer = if let Some(audio_object) = bootstrap_audio {
        let audio_handle = bootstrap_task.capability_space().install(
            audio_object,
            CapabilityRights::audio_endpoint(),
            None,
        )?;
        Some(bootstrap_task.capability_space().prepare_transfer(
            audio_handle,
            CapabilityRights::audio_endpoint(),
            TransferMode::Move,
        )?)
    } else {
        None
    };

    log_line("bootstrap", "spawning root-manager task");
    let root = kernel_user::spawn_builtin_task(
        ServiceImageId::RootManager as u32,
        TaskRole::SystemService,
        Some(root_bootstrap_transfer),
    )?;
    log_line("bootstrap", "sending root-manager startup message");
    let mut bootstrap_resource_flags = 0u64;
    if network_transfer.is_some() {
        bootstrap_resource_flags |= bootstrap_resource::NETWORK;
    }
    if display_transfer.is_some() {
        bootstrap_resource_flags |= bootstrap_resource::DISPLAY;
    }
    if input_transfer.is_some() {
        bootstrap_resource_flags |= bootstrap_resource::INPUT;
    }
    if audio_transfer.is_some() {
        bootstrap_resource_flags |= bootstrap_resource::AUDIO;
    }
    let mut startup = OutgoingMessage::new(
        MessageTag(ControlTag::Startup as u32),
        &[
            boot_store_bytes.len() as u64,
            serviceos_abi::BootstrapPlatform::QemuVirtio as u32 as u64,
            bootstrap_resource_flags,
        ],
    )?
    .add_transfer(boot_store_transfer)?
    .add_transfer(bootstrap_authority_transfer)?;
    if let Some(network_transfer) = network_transfer {
        startup = startup.add_transfer(network_transfer)?;
    }
    if let Some(display_transfer) = display_transfer {
        startup = startup.add_transfer(display_transfer)?;
    }
    if let Some(input_transfer) = input_transfer {
        startup = startup.add_transfer(input_transfer)?;
    }
    if let Some(audio_transfer) = audio_transfer {
        startup = startup.add_transfer(audio_transfer)?;
    }
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

    log_line("bootstrap", "entering userspace executor");
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
