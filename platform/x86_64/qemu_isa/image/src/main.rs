#![no_std]
#![no_main]

mod bootstrap;
mod executor;
mod logging;

use core::{fmt, panic::PanicInfo, sync::atomic::Ordering};

use serviceos_bundle::BootStore;
use serviceos_kernel_arch_x86_64::{
    cpu,
    interrupts::{self, TIMER_TICK_HZ},
    kthread,
    paging::ActivePageTable,
    smp, user,
};
use serviceos_kernel_core::{
    Kernel,
    bootstrap::{BootInfo, BootMemoryRegion, BootMemoryRegionKind},
    memory::PhysicalAddress,
    syscall, user as kernel_user,
};
use serviceos_platform_qemu_isa::serial;
use serviceos_userspace_catalog::BOOT_STORE_IMAGE;
use spin::Once;

use crate::logging::{log, log_line};

const MAX_BOOT_MEMORY_REGIONS: usize = 256;
const MAX_E820_ENTRIES: usize = 128;
const ONE_MIB: u64 = 0x10_0000;

static BOOT_STORE_IMAGE_SOURCE: Once<&'static [u8]> = Once::new();

unsafe extern "C" {
    static __payload_start: u8;
    static __payload_end: u8;
}

// The PVH entry stub lives in mb_entry.S; cargo never assembles raw .S
// files, so pull it in through the integrated assembler here. It defines
// `_start`, installs identity paging, and enters long mode calling
// `isa_entry` with the PVH hvm_start_info pointer.
core::arch::global_asm!(include_str!("mb_entry.S"), options(att_syntax));

/// QEMU PVH `hvm_start_info` (only the fields the boot path needs).
#[repr(C)]
struct HvmStartInfo {
    magic: u32,
    version: u32,
    flags: u64,
    cmdline_addr: u64,
    rsdp_paddr: u64,
    /// QEMU (version 1) reserves this slot; contents unused.
    _reserved: u64,
    memmap_paddr: u64,
    memmap_entries: u32,
    _pad: u32,
}

/// One `hvm_memmap_table_entry` (stride 16).
#[repr(C)]
struct HvmMemmapEntry {
    base: u64,
    length: u64,
    entry_type: u32,
    _reserved: u32,
}

#[unsafe(no_mangle)]
extern "C" fn isa_entry(hvm_info: *const HvmStartInfo) -> ! {
    cpu::disable_interrupts();
    serial::init();
    log_line(
        "boot",
        "entered x86_64 legacy-BIOS (SeaBIOS/PVH) kernel image",
    );
    log(
        "boot",
        format_args!(
            "payload={:#x}..{:#x} pvh-info={:#x} boot-store-bytes={}",
            payload_addr(core::ptr::addr_of!(__payload_start)),
            payload_addr(core::ptr::addr_of!(__payload_end)),
            hvm_info as u64,
            BOOT_STORE_IMAGE.len(),
        ),
    );

    // SAFETY: QEMU places the PVH start info in low memory covered by the
    // identity map before entering the 32-bit entry.
    let info = unsafe { &*hvm_info };
    // QEMU's pvh_start_info magic (include/hw/i386/x86.c): 0x336EC578.
    let (region_count, region_base) = if info.magic == 0x336EC578 {
        (
            info.memmap_entries,
            info.memmap_paddr as *const HvmMemmapEntry,
        )
    } else {
        log_line("boot", "pvh magic mismatch; continuing without memory map");
        (0, core::ptr::null())
    };
    let regions = build_memory_regions(region_count, region_base);
    for region in regions {
        log(
            "memdbg",
            format_args!(
                "{:#x}..{:#x} kind={:?}",
                region.start.as_u64(),
                region.end.as_u64(),
                region.kind
            ),
        );
    }
    let rsdp_address = scan_for_rsdp();
    match rsdp_address {
        Some(address) => log("acpi", format_args!("rsdp={}", address.as_u64())),
        None => log_line("acpi", "rsdp=not-found"),
    }

    let boot_info = BootInfo {
        memory_regions: &regions,
        memory_map_available: true,
        memory_map_truncated: region_count as usize >= MAX_BOOT_MEMORY_REGIONS,
        physical_memory_offset: Some(0),
        rsdp_address,
        framebuffer: None,
        boot_store: Some(BOOT_STORE_IMAGE),
    };

    log_line("probe", "S1");
    let mut mapper = unsafe { ActivePageTable::new_identity_mapped() };
    log_line("probe", "S2");
    let kernel = match Kernel::initialize(&boot_info, &mut mapper, TIMER_TICK_HZ as u64) {
        Ok(kernel) => kernel,
        Err(error) => panic_with_error("boot", &error),
    };
    log_line("probe", "S3");
    let descriptor_state = interrupts::initialize();
    log_line("probe", "S4");
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
    smp::bring_up_application_processors(boot_info.rsdp_address);
    kthread::spawn_pingpong_demo();
    user::initialize();
    kernel_user::initialize_runtime();
    let _ = BOOT_STORE_IMAGE_SOURCE.call_once(|| BOOT_STORE_IMAGE);
    kernel_user::register_image_resolver(resolve_boot_store_image);
    syscall::register_debug_log_writer(logging::debug_log_writer);
    syscall::register_debug_console_reader(serial::try_read_byte);
    syscall::register_debug_console_writer(serial::write_bytes);

    log_memory_summary(&kernel);

    log_line(
        "display",
        "no boot framebuffer detected (BIOS path is serial-first)",
    );
    log_line("network", "no packet interface detected");
    log_line("storage", "no writable block device detected");
    log_line("input", "no input source detected");
    log_line("audio", "no audio endpoint detected");

    let summary = match bootstrap::launch_root_manager(&kernel, None, None, None, None, None) {
        Ok(summary) => summary,
        Err(error) => panic_with_error("bootstrap", &error),
    };

    let root_thread_state = kernel
        .objects()
        .registry()
        .lookup(serviceos_kernel_core::object::ObjectId(
            summary.root_thread.0,
        ))
        .and_then(|object| object.thread().map(|thread| thread.snapshot()));

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

    cpu::halt_loop()
}

fn build_memory_regions(count: u32, buffer: *const HvmMemmapEntry) -> &'static [BootMemoryRegion] {
    static REGIONS: spin::Mutex<[BootMemoryRegion; MAX_BOOT_MEMORY_REGIONS]> =
        spin::Mutex::new([BootMemoryRegion::EMPTY; MAX_BOOT_MEMORY_REGIONS]);
    static LEN: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

    // SAFETY: the PVH loader wrote `count` stride-16 memory-map records at
    // `buffer`; the identity map makes them readable here.
    let entries = if count == 0 || buffer.is_null() {
        &[]
    } else {
        unsafe { core::slice::from_raw_parts(buffer, count.min(MAX_E820_ENTRIES as u32) as usize) }
    };
    let payload_start = payload_addr(core::ptr::addr_of!(__payload_start));
    let payload_end = payload_addr(core::ptr::addr_of!(__payload_end));

    let mut guard = REGIONS.lock();
    let mut written = 0usize;
    for entry in entries {
        if written >= guard.len() {
            break;
        }
        let start = entry.base;
        let end = entry.base.saturating_add(entry.length);
        if end <= start {
            continue;
        }
        let kind = match entry.entry_type {
            1 => {
                if end <= ONE_MIB {
                    // Legacy low memory hosts stage1 scratch, page tables and
                    // the boot stack remnants; keep it out of the frame pool.
                    BootMemoryRegionKind::BootloaderOwned
                } else {
                    BootMemoryRegionKind::Usable
                }
            }
            3 => BootMemoryRegionKind::AcpiReclaimable,
            4 => BootMemoryRegionKind::FirmwareReserved,
            _ => BootMemoryRegionKind::Reserved,
        };
        let mut region_start = start.max(ONE_MIB.min(end));
        if kind == BootMemoryRegionKind::Usable && region_start < payload_end && end > payload_start
        {
            // Exclude the loaded kernel image from usable frames.
            region_start = region_start.max(align_up(payload_end, 0x1000));
            if region_start >= end {
                continue;
            }
        }
        guard[written] = BootMemoryRegion {
            start: PhysicalAddress::new(region_start),
            end: PhysicalAddress::new(end),
            kind,
        };
        written += 1;
    }
    LEN.store(written, Ordering::Release);
    unsafe { core::slice::from_raw_parts(guard.as_ptr(), written) }
}

fn scan_for_rsdp() -> Option<PhysicalAddress> {
    // EBDA pointer at BDA 0x40E (segment), then the BIOS area 0xE0000..0xFFFFF.
    let ebda_segment = unsafe { read_u16(0x40E) } as usize;
    if ebda_segment >= 0x8000 {
        let base = ebda_segment * 16;
        for offset in (0..0x400).step_by(16) {
            if let Some(address) = try_rsdp_at(base + offset) {
                return Some(address);
            }
        }
    }
    for address in (0xE0000..0x100000).step_by(16) {
        if let Some(found) = try_rsdp_at(address) {
            return Some(found);
        }
    }
    None
}

fn try_rsdp_at(address: usize) -> Option<PhysicalAddress> {
    const RSDP_SIGNATURE: &[u8; 8] = b"RSD PTR ";
    let mut header = [0u8; 20];
    for (index, byte) in header.iter_mut().enumerate() {
        // SAFETY: identity-mapped BIOS/EBDA ranges; faults impossible there on pc.
        *byte = unsafe { read_u8(address + index) };
    }
    if &header[..8] != RSDP_SIGNATURE {
        return None;
    }
    if header[15] != 0 && header[15] < 2 {
        return None;
    }
    let checksum = header.iter().fold(0u8, |acc, byte| acc.wrapping_add(*byte));
    if checksum != 0 {
        return None;
    }
    if header[15] >= 2 {
        let mut extended = [0u8; 36];
        for (index, byte) in extended.iter_mut().enumerate() {
            // SAFETY: same identity-mapped firmware range as above.
            *byte = unsafe { read_u8(address + index) };
        }
        let extended_checksum = extended
            .iter()
            .skip(20)
            .fold(0u8, |acc, byte| acc.wrapping_add(*byte));
        if extended_checksum != 0 {
            return None;
        }
    }
    Some(PhysicalAddress::new(address as u64))
}

fn log_memory_summary(kernel: &Kernel<'_>) {
    log(
        "memory",
        format_args!(
            "regions={} usable={} boot-store-bytes={}",
            kernel.boot_context().memory_region_count(),
            kernel.boot_context().usable_memory_region_count(),
            BOOT_STORE_IMAGE.len(),
        ),
    );
    log(
        "memory",
        format_args!(
            "root-page-table={:#x} heap={:#x}..{:#x} usable-mib={} remaining-mib={}",
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
        ),
    );
}

fn resolve_boot_store_image(image_id: u32) -> Option<&'static [u8]> {
    let boot_store = BOOT_STORE_IMAGE_SOURCE.get().copied()?;
    BootStore::parse(boot_store).ok()?.resolve_image(image_id)
}

fn payload_addr(symbol: *const u8) -> u64 {
    symbol as u64
}

fn align_up(value: u64, alignment: u64) -> u64 {
    value.div_ceil(alignment) * alignment
}

// SAFETY: callers pass fixed BIOS data-area addresses that are always mapped
// by the stage1 identity map.
unsafe fn read_u8(address: usize) -> u8 {
    unsafe { core::ptr::read_volatile(address as *const u8) }
}

// SAFETY: see read_u8.
unsafe fn read_u16(address: usize) -> u16 {
    unsafe { core::ptr::read_volatile(address as *const u16) }
}

fn panic_with_error(scope: &str, error: &impl fmt::Debug) -> ! {
    log(scope, format_args!("bring-up failed: {error:?}"));
    cpu::halt_loop()
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    log("panic", format_args!("{info}"));
    cpu::halt_loop()
}
