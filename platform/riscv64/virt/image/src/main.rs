#![no_main]
#![no_std]

use core::{fmt::Write, panic::PanicInfo};

use serviceos_kernel_arch_riscv64::{console::SbiConsole, cpu, layout, timer, traps};
use serviceos_platform_riscv64_virt::machine;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".boot_stack")]
static mut BOOT_STACK: [u8; 64 * 1024] = [0; 64 * 1024];

core::arch::global_asm!(
    ".section .text._start, \"ax\"",
    ".globl _start",
    ".type _start, %function",
    "_start:",
    "bnez a0, 4f",
    "la sp, __boot_stack_top",
    "la t0, __bss_start",
    "la t1, __bss_end",
    "1:",
    "bgeu t0, t1, 3f",
    "sd zero, 0(t0)",
    "addi t0, t0, 8",
    "j 1b",
    "3:",
    "call rust_main",
    "4:",
    "wfi",
    "j 4b",
    ".section .text",
);

#[unsafe(no_mangle)]
extern "C" fn rust_main(hart_id: usize, dtb_pointer: usize) -> ! {
    let mut console = SbiConsole::new();
    let _ = writeln!(
        console,
        "serviceos: riscv64 virt stub alive (hart {}, dtb {:#x}, kernel at {:#x})",
        hart_id,
        dtb_pointer,
        layout::KERNEL_LOAD_BASE
    );

    traps::init();
    let _ = writeln!(
        console,
        "serviceos: stvec armed with all-traps hang handler"
    );

    match timer::arm_oneshot_tick(timer::TIMER_TICK_HZ) {
        Ok(interval) => {
            let _ = writeln!(
                console,
                "serviceos: sbi set_timer armed, interval {} ticks @ {} Hz timebase",
                interval,
                timer::QEMU_VIRT_TIMEBASE_HZ
            );
        }
        Err(error) => {
            let _ = writeln!(console, "serviceos: timer arm failed: {:?}", error);
        }
    }
    let _ = writeln!(
        console,
        "serviceos: counter reads {} (interrupts stay masked in this skeleton)",
        timer::now()
    );
    let _ = writeln!(
        console,
        "serviceos: skeleton scope note - bare-metal identity map, MMU off, no userspace; parking hart 0"
    );

    cpu::park()
}

fn print_panic(console: &mut SbiConsole, info: &PanicInfo<'_>) {
    let _ = writeln!(console, "serviceos: riscv64 skeleton panic: {}", info);
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    let mut console = SbiConsole::new();
    print_panic(&mut console, info);
    machine::qemu_exit(machine::FINISHER_FAIL)
}
