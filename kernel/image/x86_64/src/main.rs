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
use serviceos_kernel_core::Kernel;
use uefi::{Status, entry};

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
            serial::write_line("serviceos: phase2 control-flow foundation initialized; halting");
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
