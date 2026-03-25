#![no_main]
#![no_std]

use core::panic::PanicInfo;
use serviceos_kernel_arch_x86_64::{boot::boot_context_from_uefi, cpu, serial};
use serviceos_kernel_core::Kernel;
use uefi::{Status, entry};

#[entry]
fn kernel_main() -> Status {
    cpu::disable_interrupts();
    serial::init();
    serial::write_line("serviceos: entered x86_64 UEFI kernel image");

    let boot_context = boot_context_from_uefi();
    let kernel = Kernel::initialize(&boot_context);

    serial::write_args(format_args!(
        "serviceos: boot memory map available = {}\n",
        kernel.boot_context().memory_map_available
    ));
    serial::write_line("serviceos: phase0 skeleton initialized; halting");

    cpu::halt_loop()
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    serial::write_args(format_args!("serviceos: panic: {info}\n"));
    cpu::halt_loop()
}
