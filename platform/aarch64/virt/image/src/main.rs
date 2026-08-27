#![no_main]
#![no_std]

extern crate alloc;

use core::{
    fmt,
    fmt::Write,
    panic::PanicInfo,
    str,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

use serviceos_abi::{BootstrapPlatform, ControlTag, ServiceImageId, bootstrap_resource};
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
    object::{KernelObjectRef, ObjectId},
    syscall,
    task::{self, ExecutionState, SchedulerError, TaskRole, ThreadId, ThreadMode},
    user::{self as kernel_user, SpawnError, TaskExitStatus},
};
use serviceos_platform_virt::{
    audio, block, boot, dtb::InterruptControllerRegions, framebuffer, input, net, selftest, timer,
    uart,
};
use serviceos_userspace_catalog::BOOT_STORE_IMAGE;
use spin::{Mutex, Once};

const TIMER_TICK_HZ: u64 = 100;
const MAX_MMIO_REGIONS: usize = 40;

static HARDWARE_TICKS: AtomicBool = AtomicBool::new(false);

// Device IRQ ack table: INTID -> virtio-mmio register base. The GIC hook
// runs at IRQ context with interrupts masked, so it only touches device MMIO
// (InterruptStatus read + InterruptAck write, register offsets shared by
// virtio-mmio v1 and v2) and never driver locks; the executor's poll pass
// then drains the completed work.
const VIRTIO_MMIO_INTERRUPT_STATUS: u64 = 0x60;
const VIRTIO_MMIO_INTERRUPT_ACK: u64 = 0x64;
const MAX_DEVICE_IRQS: usize = 32;
static DEVICE_IRQ_BASES: Mutex<[(u16, u64); MAX_DEVICE_IRQS]> =
    Mutex::new([(0, 0); MAX_DEVICE_IRQS]);
static DEVICE_IRQS_ACKED: AtomicU64 = AtomicU64::new(0);

fn register_device_irq_base(intid: u16, mmio_base: u64) {
    let mut table = DEVICE_IRQ_BASES.lock();
    for entry in table.iter_mut() {
        if entry.0 == intid {
            return;
        }
        if entry.0 == 0 {
            *entry = (intid, mmio_base);
            return;
        }
    }
}

fn virtio_device_irq_hook(intid: u16) {
    let table = DEVICE_IRQ_BASES.lock();
    if let Some((_, base)) = table.iter().find(|entry| entry.0 == intid && entry.1 != 0) {
        let status =
            unsafe { core::ptr::read_volatile((base + VIRTIO_MMIO_INTERRUPT_STATUS) as *const u32) };
        // Ack every bit the device reports: the ISR can carry more than the
        // two standard used/config bits under QEMU, and leftover bits keep a
        // level line asserted forever.
        unsafe {
            core::ptr::write_volatile((base + VIRTIO_MMIO_INTERRUPT_ACK) as *mut u32, status);
        }
        drop(table);
        DEVICE_IRQS_ACKED.fetch_add(1, Ordering::Relaxed);
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".boot_stack")]
static mut BOOT_STACK: [u8; 512 * 1024] = [0; 512 * 1024];

core::arch::global_asm!(
    ".section .image.header, \"ax\"",
    ".globl serviceos_kernel_image_header",
    "serviceos_kernel_image_header:",
    "b _start",
    "nop",
    ".quad 0x00080000",
    ".quad {image_size}",
    ".quad 0x0",
    ".quad 0x0",
    ".quad 0x0",
    ".quad 0x0",
    ".byte 0x41, 0x52, 0x4d, 0x64",
    ".byte 0x0, 0x0, 0x0, 0x0",
    ".section .text._start, \"ax\"",
    ".globl _start",
    ".type _start, %function",
    "_start:",
    "mov x19, x0",
    "mrs x20, mpidr_el1",
    "tst x20, #0xff",
    "b.ne 1f",
    "mrs x21, CurrentEL",
    "lsr x21, x21, #2",
    "cmp x21, #2",
    "b.ne 2f",
    "mov x22, #(1 << 31)",
    "msr hcr_el2, x22",
    "mrs x22, cnthctl_el2",
    "orr x22, x22, #0x3",
    "msr cnthctl_el2, x22",
    "msr cntvoff_el2, xzr",
    "dsb sy",
    "isb",
    "mov x22, #0x3c5",
    "msr spsr_el2, x22",
    "adr x22, 2f",
    "msr elr_el2, x22",
    "eret",
    "2:",
    "msr spsel, #1",
    "adrp x0, {stack}",
    "add x0, x0, :lo12:{stack}",
    "mov x1, {size}",    "add x0, x0, x1",
    "mov sp, x0",
    "mov x0, x19",
    "b {entry}",
    "1:",
    "wfe",
    "b 1b",
    size = const 512 * 1024,
    stack = sym BOOT_STACK,
    entry = sym serviceos_virt_entry,
    image_size = sym __kernel_image_size,
);

unsafe extern "C" {
    static __image_start: u8;
    static __image_end: u8;
    static __kernel_image_size: u8;
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
extern "C" fn serviceos_virt_entry(dtb_ptr: usize) -> ! {
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

    log_line("boot", "entered QEMU virt machine kernel image");
    log(
        "boot",
        format_args!(
            "machine=qemu-virt model={} compatible={} current-el={} core={}",
            boot_state.summary.model,
            boot_state.summary.compatible.unwrap_or("unknown"),
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
    }; MAX_MMIO_REGIONS];
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
    for device in boot_state.summary.virtio_mmio_devices[..boot_state.summary.virtio_mmio_count]
        .iter()
        .copied()
        .filter(|device| device.is_populated())
    {
        if mmio_region_count == mmio_regions.len() {
            break;
        }
        log(
            "virtio",
            format_args!(
                "mmio-slot base={:#x} size={} irq={}",
                device.base.as_u64(),
                device.size,
                device.irq,
            ),
        );
        mmio_regions[mmio_region_count] = MmioRegion {
            base: device.base,
            size: device.size,
        };
        mmio_region_count += 1;
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

    let virtio_devices =
        &boot_state.summary.virtio_mmio_devices[..boot_state.summary.virtio_mmio_count];
    let block_backend = block::initialize(virtio_devices);
    let network_backend = net::initialize(virtio_devices);
    // Register the lock-free device IRQ ack hook BEFORE enabling any device
    // SPI: with the SPIs enabled and nobody acking the device line, the
    // first completion would re-fire forever as an IRQ storm.
    for device in virtio_devices.iter().copied().filter(|d| d.is_populated()) {
        if device.irq >= 32 && device.irq <= u32::from(u16::MAX) {
            register_device_irq_base(device.irq as u16, device.base.as_u64());
        }
    }
    serviceos_kernel_core::interrupts::register_external_irq_hook(virtio_device_irq_hook);
    let display_backend = framebuffer::initialize(virtio_devices);
    let input_backend = input::initialize(virtio_devices);
    let bootstrap_block = block_backend
        .clone()
        .map(|backend| kernel.objects().registry().create_block_device(backend));
    let bootstrap_network = network_backend
        .clone()
        .map(|backend| kernel.objects().registry().create_packet_interface(backend));
    let bootstrap_display = display_backend
        .clone()
        .map(|backend| kernel.objects().registry().create_display_output(backend));
    let bootstrap_input = input_backend
        .clone()
        .map(|backend| kernel.objects().registry().create_input_source(backend));
    let bootstrap_audio = Some(
        kernel
            .objects()
            .registry()
            .create_audio_endpoint(audio::initialize()),
    );

    if let Some(summary) = block::bringup_summary() {
        log(
            "storage",
            format_args!(
                "block-backend={:?} mmio-base={:#x} irq={} blocks={} block-size={} writable={}",
                summary.backend,
                summary.mmio_base,
                summary.irq,
                summary.block_count,
                summary.block_size,
                summary.writable,
            ),
        );
    } else {
        log_line("storage", "no virtio-blk device detected");
    }
    if let Some(summary) = net::bringup_summary() {
        log(
            "network",
            format_args!(
                "backend={:?} mmio-base={:#x} irq={} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} mtu={}",
                summary.backend,
                summary.mmio_base,
                summary.irq,
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
        log_line("network", "no virtio-net device detected");
    }
    if let Some(summary) = framebuffer::bringup_summary() {
        log(
            "display",
            format_args!(
                "backend=virtio-gpu mmio-base={:#x} {}x{} stride-bytes={} bytes={}",
                summary.mmio_base,
                summary.width,
                summary.height,
                summary.stride_bytes,
                summary.byte_len,
            ),
        );
    } else {
        log_line("display", "no virtio-gpu device detected");
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
        log_line("input", "no virtio-input device detected");
    }
    log(
        "audio",
        format_args!(
            "backend={:?} null-sink pcm-soft-mix-only",
            serviceos_abi::AudioEndpointBackend::Unknown
        ),
    );

    log_memory_summary(&boot_state.boot_info);
    log(
        "timer",
        format_args!(
            "backend=arm-generic freq={} tick-hz={}",
            timer::counter_frequency_hz(),
            TIMER_TICK_HZ,
        ),
    );
    match bring_up_interrupts(
        boot_state.summary.interrupt_controller,
        boot_state.summary.timer_ppi_intid,
        virtio_devices,
    ) {
        Ok(interval_cycles) => {
            HARDWARE_TICKS.store(true, Ordering::Relaxed);
            log(
                "interrupts",
                format_args!(
                    "backend=gic-v3 timer=el1-physical ppi={} tick-hz={} interval-cycles={}",
                    gic::timer_ppi_intid(),
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
    log_line("bootstrap", "device selftests starting");
    selftest::run_all(
        &block_backend,
        &network_backend,
        &display_backend,
        &input_backend,
    );
    log_line("bootstrap", "starting serial-first userspace graph");

    let summary = match launch_root_manager(
        &kernel,
        bootstrap_block,
        bootstrap_network,
        bootstrap_display,
        bootstrap_input,
        bootstrap_audio,
    ) {
        Ok(summary) => summary,
        Err(error) => panic_with_error("bootstrap", error),
    };

    // A cleanly exited root thread drops its last strong reference and the
    // registry garbage-collects it, so a live snapshot only exists while the
    // root task is still running (or faulted without exiting).
    let root_thread_state = kernel
        .objects()
        .registry()
        .lookup(ObjectId(summary.root_thread.0))
        .and_then(|object| object.thread().map(|thread| thread.snapshot()));
    if root_thread_state.is_none() && matches!(summary.exit_status, TaskExitStatus::Running) {
        panic_with_error("bootstrap", BootstrapError::MissingRootThread);
    }

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
    if let Some(state) = root_thread_state {
        log(
            "userspace",
            format_args!(
                "root-thread mode={:?} state={:?} wait={:?} wake={:?}",
                state.mode, state.execution_state, state.wait_target, state.last_wake_reason,
            ),
        );
    }
    log_line(
        "bootstrap",
        "root userspace service graph completed; halting",
    );
    cpu::wait_forever()
}

fn transfer_bootstrap_object(
    bootstrap_task: &serviceos_kernel_core::task::TaskObject,
    object: Option<KernelObjectRef>,
    rights: CapabilityRights,
) -> Result<Option<serviceos_kernel_core::capability::PreparedTransfer>, BootstrapError> {
    let Some(object) = object else {
        return Ok(None);
    };
    let handle = bootstrap_task
        .capability_space()
        .install(object, rights, None)?;
    Ok(Some(bootstrap_task.capability_space().prepare_transfer(
        handle,
        rights,
        TransferMode::Move,
    )?))
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    log("panic", format_args!("{info}"));
    cpu::wait_forever()
}

use serviceos_kernel_core::memory::PhysicalAddress;
/// Boot-mode word passed to the root-manager in the startup message
/// (3 = recovery; see root-manager bootmode). Selected at build time via
/// SERVICEOS_BOOT_MODE=recovery, e.g. `cargo xtask recover`.
fn root_boot_mode_word() -> u64 {
    if option_env!("SERVICEOS_BOOT_MODE") == Some("recovery") {
        3
    } else {
        0
    }
}

fn launch_root_manager(
    kernel: &Kernel<'_>,
    bootstrap_block: Option<KernelObjectRef>,
    bootstrap_network: Option<KernelObjectRef>,
    bootstrap_display: Option<KernelObjectRef>,
    bootstrap_input: Option<KernelObjectRef>,
    bootstrap_audio: Option<KernelObjectRef>,
) -> Result<RootBootstrapSummary, BootstrapError> {
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
    let block_transfer = transfer_bootstrap_object(
        bootstrap_task,
        bootstrap_block,
        CapabilityRights::block_device(),
    )?;
    let network_transfer = transfer_bootstrap_object(
        bootstrap_task,
        bootstrap_network,
        CapabilityRights::packet_interface(),
    )?;
    let display_transfer = transfer_bootstrap_object(
        bootstrap_task,
        bootstrap_display,
        CapabilityRights::display_output(),
    )?;
    let input_transfer = transfer_bootstrap_object(
        bootstrap_task,
        bootstrap_input,
        CapabilityRights::input_source(),
    )?;
    let audio_transfer = transfer_bootstrap_object(
        bootstrap_task,
        bootstrap_audio,
        CapabilityRights::audio_endpoint(),
    )?;

    let root = kernel_user::spawn_builtin_task(
        ServiceImageId::RootManager as u32,
        TaskRole::SystemService,
        Some(root_bootstrap_transfer),
    )?;
    let mut bootstrap_resource_flags = 0u64;
    if block_transfer.is_some() {
        bootstrap_resource_flags |= bootstrap_resource::BLOCK;
    }
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
            BootstrapPlatform::QemuVirtio as u32 as u64,
            bootstrap_resource_flags,
            root_boot_mode_word(),
        ],
    )?
    .add_transfer(boot_store_transfer)?
    .add_transfer(bootstrap_authority_transfer)?;
    if let Some(block_transfer) = block_transfer {
        startup = startup.add_transfer(block_transfer)?;
    }
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
        .ok_or_else(|| {
            log(
                "bootstrap",
                format_args!("DBG spawn: thread object missing"),
            );
            BootstrapError::MissingRootThread
        })?
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
    timer_ppi_intid: Option<u16>,
    virtio_devices: &[serviceos_platform_virt::dtb::VirtioMmioDevice],
) -> Result<u64, &'static str> {
    // The device tree names the EL1 non-secure physical timer PPI; arming
    // `cntp_cval_el0` asserts that INTID, not the secure-physical PPI 29.
    if let Some(intid) = timer_ppi_intid {
        gic::set_timer_ppi_intid(intid);
    }
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
    // Device IRQ enable path: GIC init masks every SPI, so each populated
    // virtio-mmio INTID (device tree already decodes SPI numbers to INTIDs)
    // must be enabled here before any device interrupt can be delivered.
    for device in virtio_devices.iter().copied().filter(|d| d.is_populated()) {
        if device.irq >= 32 && device.irq <= u32::from(u16::MAX) {
            gic::enable_spi(device.irq as u16);
        }
    }
    kernel_timer::arm_periodic_tick(TIMER_TICK_HZ)
        .map_err(|_| "timer-counter-frequency-unavailable")
}

fn run_userspace_executor(
    kernel: &Kernel<'_>,
    root_task: serviceos_kernel_core::task::TaskId,
) -> Result<(), BootstrapError> {
    let hardware_ticks = HARDWARE_TICKS.load(Ordering::Relaxed);
    let mut timer_state = initialize_timer_poll_state();
    let mut executor_iterations: u64 = 0;
    loop {
        executor_iterations += 1;
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
        poll_device_events();
        // The per-iteration executor trace is debug-grade noise under TCG
        // (UART per loop pass dominates the schedule); emit it at a coarse
        // cadence instead so boot logs stay readable and the loop keeps up
        // with poll-drain delivery.
        if executor_iterations % 4096 == 0 {
            log(
                "executor",
                format_args!(
                    "DBG iter cur={:?} run={} blk={} sw={}",
                    current.map(|thread| thread.0),
                    snapshot.runnable_threads,
                    snapshot.blocked_threads,
                    snapshot.context_switches,
                ),
            );
        }
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
            log_line("executor", "DBG no-current: scheduler idle");
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
            log(
                "executor",
                format_args!(
                    "DBG lookup-failed tid={} run={} blk={}",
                    thread_id.0, snapshot.runnable_threads, snapshot.blocked_threads
                ),
            );
            return Err(BootstrapError::MissingRootThread);
        };
        let Some(thread_state) = thread_object.thread().map(|thread| thread.snapshot()) else {
            log(
                "executor",
                format_args!("DBG not-a-thread tid={}", thread_id.0),
            );
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

fn poll_device_events() {
    input::poll_ready_sources();
    if let Some(manager) = serviceos_kernel_core::network::manager() {
        manager.poll_ready(|object_id| {
            let _ = task::notify_packet_ready(ObjectId(object_id));
        });
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
