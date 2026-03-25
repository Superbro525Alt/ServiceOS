#![no_main]
#![no_std]

use core::panic::PanicInfo;
use serviceos_kernel_arch_x86_64::{boot::exit_boot_services_and_capture_context, cpu, serial};
use serviceos_kernel_core::Kernel;
use uefi::{Status, entry};

#[entry]
fn kernel_main() -> Status {
    cpu::disable_interrupts();
    serial::init();
    serial::write_line("serviceos: entered x86_64 UEFI kernel image");

    let boot_context = exit_boot_services_and_capture_context();
    let kernel = Kernel::initialize(&boot_context);

    serial::write_args(format_args!(
        "serviceos: memory regions = {} (usable = {}, boot-services reclaimable = {})\n",
        kernel.boot_context().memory_region_count(),
        kernel.boot_context().usable_memory_region_count(),
        kernel
            .boot_context()
            .boot_services_reclaimable_region_count()
    ));
    serial::write_line("serviceos: phase0 skeleton initialized; halting");

    cpu::halt_loop()
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    serial::write_args(format_args!("serviceos: panic: {info}\n"));
    cpu::halt_loop()
}
