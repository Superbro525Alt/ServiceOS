#![no_main]
#![no_std]

use core::panic::PanicInfo;
use serviceos_kernel_arch_x86_64::{
    boot::exit_boot_services_and_capture_context, cpu, paging::ActivePageTable, serial,
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
    let kernel = match Kernel::initialize(&boot_context, &mut mapper) {
        Ok(kernel) => kernel,
        Err(error) => {
            serial::write_args(format_args!("serviceos: kernel init failed: {error:?}\n"));
            cpu::halt_loop()
        }
    };
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
    serial::write_line("serviceos: phase1 memory foundation initialized; halting");

    cpu::halt_loop()
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    serial::write_args(format_args!("serviceos: panic: {info}\n"));
    cpu::halt_loop()
}
