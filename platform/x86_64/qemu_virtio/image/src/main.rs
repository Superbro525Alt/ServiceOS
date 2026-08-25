#![no_main]
#![no_std]

mod bootstrap;
mod executor;
mod logging;

use core::panic::PanicInfo;

use bootstrap::{BootstrapError, launch_root_manager, resolve_boot_store_image};
use logging::{debug_log_writer, log, log_line};
use serviceos_kernel_arch_x86_64::{
    cpu,
    interrupts::{self, TIMER_TICK_HZ},
    kthread,
    paging::ActivePageTable,
    smp, user,
};
use serviceos_kernel_core::{Kernel, syscall, user as kernel_user};
use serviceos_platform_qemu_virtio::{audio, block, boot, display, input, network, serial, sound};
use spin::Once;
use uefi::{Status, entry};

static BOOT_STORE_IMAGE_SOURCE: Once<&'static [u8]> = Once::new();

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
    smp::bring_up_application_processors(boot_info.rsdp_address);
    // Second kernel-thread wave for the APs to steal.
    kthread::spawn_pingpong_demo();
    user::initialize();
    kernel_user::initialize_runtime();
    let _ = BOOT_STORE_IMAGE_SOURCE.call_once(|| boot_store);
    kernel_user::register_image_resolver(resolve_boot_store_image);
    syscall::register_debug_log_writer(debug_log_writer);
    syscall::register_debug_console_reader(serial::try_read_byte);
    syscall::register_debug_console_writer(serial::write_bytes);

    let bootstrap_network = network::initialize()
        .map(|backend| kernel.objects().registry().create_packet_interface(backend));
    let bootstrap_block =
        block::initialize().map(|backend| kernel.objects().registry().create_block_device(backend));
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
    if let Some(summary) = block::bringup_summary() {
        log(
            "storage",
            format_args!(
                "block-backend={:?} pci={:02x}:{:02x}.{} blocks={} block-size={} writable={}",
                summary.backend,
                summary.pci_bus,
                summary.pci_device,
                summary.pci_function,
                summary.block_count,
                summary.block_size,
                summary.writable,
            ),
        );
    } else {
        log_line("storage", "no writable block device detected");
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
        for instance in summary.instances[..summary.instance_count].iter() {
            log(
                "input",
                format_args!(
                    "instance id={} class={:#x} role_flags={:#x}",
                    instance.source_id, instance.class, instance.role_flags,
                ),
            );
        }
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
    if let Some(summary) = sound::bringup_summary() {
        log(
            "audio",
            format_args!(
                "pcm-sink=virtio-sound pci={:02x}:{:02x}.{} stream={} rate={}Hz channels={}",
                summary.pci_bus,
                summary.pci_device,
                summary.pci_function,
                summary.stream_id,
                summary.rate_hz,
                summary.channels,
            ),
        );
    }

    let summary = match launch_root_manager(
        &kernel,
        bootstrap_block,
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
        .lookup(serviceos_kernel_core::object::ObjectId(
            summary.root_thread.0,
        ))
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
