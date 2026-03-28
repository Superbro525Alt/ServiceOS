#![no_main]
#![no_std]

use core::{fmt, fmt::Write, panic::PanicInfo};

use serviceos_kernel_arch_aarch64::cpu;
use serviceos_kernel_core::{bootstrap::BootInfo, memory::PhysicalAddress};
use serviceos_platform_raspi5::{boot, uart};

#[unsafe(no_mangle)]
#[unsafe(link_section = ".boot_stack")]
static mut BOOT_STACK: [u8; 64 * 1024] = [0; 64 * 1024];

unsafe extern "C" {
    static __image_start: u8;
    static __image_end: u8;
}

#[unsafe(no_mangle)]
extern "C" fn serviceos_raspi5_entry(dtb_ptr: usize) -> ! {
    let kernel_start = PhysicalAddress::new(core::ptr::addr_of!(__image_start) as u64);
    let kernel_end = PhysicalAddress::new(core::ptr::addr_of!(__image_end) as u64);
    let boot_state = match boot::capture_boot_info(dtb_ptr as *const u8, kernel_start, kernel_end) {
        Ok(state) => state,
        Err(error) => panic_with_error("boot", error),
    };

    if let Some(descriptor) = boot_state.summary.uart {
        uart::initialize(descriptor);
    }

    log_line("boot", "entered Raspberry Pi 5 kernel image");
    log(
        "boot",
        format_args!(
            "model={} compatible={} serial={}",
            boot_state.summary.model,
            boot_state.summary.compatible.unwrap_or("unknown"),
            boot_state.summary.serial_number.unwrap_or("unknown"),
        ),
    );
    log(
        "boot",
        format_args!(
            "dtb-base={:#x} dtb-bytes={} current-el={} core={}",
            boot_state.summary.dtb_base.as_u64(),
            boot_state.summary.dtb_size,
            cpu::current_el(),
            cpu::core_id(),
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
    } else {
        log_line("serial", "stdout UART not discovered from device tree");
    }
    log_memory_summary(&boot_state.boot_info);
    log_line("display", "raspi5 framebuffer backend deferred");
    log_line("network", "raspi5 packet backend deferred");
    log_line(
        "bootstrap",
        "raspi5 target currently stops after native arch/platform bring-up",
    );

    cpu::wait_forever()
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    log("panic", format_args!("{info}"));
    cpu::wait_forever()
}

fn log_memory_summary(boot_info: &BootInfo<'_>) {
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

    for (index, region) in boot_info.memory_regions.iter().take(4).enumerate() {
        log(
            "memory",
            format_args!(
                "region[{index}] start={:#x} end={:#x} kind={:?}",
                region.start.as_u64(),
                region.end.as_u64(),
                region.kind,
            ),
        );
    }
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
