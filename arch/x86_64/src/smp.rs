use core::arch::global_asm;
use core::sync::atomic::{AtomicU32, Ordering};

use alloc::vec::Vec;
use serviceos_kernel_core::memory::PhysicalAddress;

use crate::{acpi, interrupts, kthread, lapic, serial};

/// Physical page the SMP trampoline is copied into. Must be below 1 MiB,
/// clear of the real-mode IVT (0x0-0x400) and the EBDA (>= 0x9FC00 on
/// QEMU), and identity-mapped — which every physical address is under this
/// kernel's offset-0 direct map.
const TRAMPOLINE_PHYSICAL: u64 = 0x7000;
const SIPI_VECTOR: u8 = (TRAMPOLINE_PHYSICAL >> 12) as u8;

/// Parameter slots inside the trampoline page, consumed by AP assembly.
const PARAM_CR3: u64 = TRAMPOLINE_PHYSICAL + 0x200;
const PARAM_RSP: u64 = TRAMPOLINE_PHYSICAL + 0x208;
const PARAM_TARGET: u64 = TRAMPOLINE_PHYSICAL + 0x210;
const PARAM_CPU_ID: u64 = TRAMPOLINE_PHYSICAL + 0x218;

/// Upper bound on brought-up APs; extra MADT entries are ignored.
const MAX_SMP_CPUS: usize = 4;

/// INIT-SIPI sequencing windows from the multi-processor specification.
const INIT_DEASSERT_DELAY_MS: u32 = 10;
const STARTUP_ONLINE_TIMEOUT_MS: u32 = 200;

#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct ApStack([u8; AP_STACK_BYTES]);
const AP_STACK_BYTES: usize = 16 * 1024;

static AP_STACKS: [ApStack; MAX_SMP_CPUS] = [ApStack([0; AP_STACK_BYTES]); MAX_SMP_CPUS];
static APS_ONLINE: AtomicU32 = AtomicU32::new(0);
static APS_EXPECTED: AtomicU32 = AtomicU32::new(0);

// The trampoline is assembled position-fixed for its destination page:
// every reference folds `label - smp_trampoline_start` into an assembly-time
// constant that `+ TRAMPOLINE_PHYS` turns into the runtime address, because
// link-time addresses differ from the low page SIPI starts executing at.
// Far jumps and LGDT are hand-encoded so their immediates use those folded
// constants too.
global_asm!(
    r#"
.section .smp_trampoline, "ax"
.code16
smp_trampoline_start:
    cli
    movw $0x03f8, %dx                 /* COM1: stage marker 'A' (real mode) */
    movb $0x41, %al
    outb %al, %dx
    xorw %ax, %ax
    movw %ax, %ds
    movw %ax, %ss
    movw ${TRAMP_PHYS}, %sp          /* stack grows down below the page */
    .byte 0x0f, 0x01, 0x16           /* lgdt ds:[disp16] */
    .word gdtr - smp_trampoline_start + {TRAMP_PHYS}
    movb $0x61, %al                  /* 'a': lgdt survived */
    outb %al, %dx
    movl %cr0, %eax
    orl $0x00000001, %eax            /* PE: real mode -> protected mode */
    movl %eax, %cr0
    .byte 0xea                       /* ljmp $0x08:$pm32 */
    .word pm32 - smp_trampoline_start + {TRAMP_PHYS}
    .word 0x0008

.code32
pm32:
    /* Data segments still hold real-mode selector 0 (null) — reload them
       before any memory access or the first operand fetch #GP-faults. */
    movw $0x0010, %ax
    movw %ax, %ds
    movw %ax, %ss
    movw %ax, %es
    movw $0x03f8, %dx                 /* stage marker 'B' (32-bit mode) */
    movb $0x42, %al
    outb %al, %dx
    movl %cr4, %eax
    orl $0x20, %eax                  /* PAE */
    movl %eax, %cr4
    movl ({CR3_SLOT}), %eax         /* adopt the BSP's identity-mapped root */
    movl %eax, %cr3
    movl $0xc0000080, %ecx           /* IA32_EFER */
    rdmsr
    orl $0x900, %eax                 /* LME | NXE: INIT reset EFER on this CPU,
                                        so NXE must be re-enabled before any
                                        walk of NX-tagged kernel heap pages */
    wrmsr
    movl %cr0, %eax
    orl $0x80000000, %eax            /* PG (+ PE already set); LMA follows */
    movl %eax, %cr0
    .byte 0xea                       /* ljmp $0x18:$lm64 */
    .long lm64 - smp_trampoline_start + {TRAMP_PHYS}
    .word 0x0018

.code64
lm64:
    movw $0x0010, %ax
    movw %ax, %ds
    movw %ax, %es
    movw %ax, %ss
    movw $0x03f8, %dx                 /* stage marker 'C' (64-bit mode) */
    movb $0x43, %al
    outb %al, %dx
    movq ${RSP_SLOT}, %rdi
    movq (%rdi), %rsp                /* dedicated AP kernel stack */
    movq ${CPU_ID_SLOT}, %rcx
    movq (%rcx), %rcx                /* Microsoft x64 arg0 = cpu id */
    movq ${TARGET_SLOT}, %rax
    jmpq *(%rax)                     /* into ap_main, never returns */

.balign 8
gdt:
    .quad 0x0000000000000000         /* null */
    .quad 0x00cf9a000000ffff         /* 0x08: 32-bit code */
    .quad 0x00cf92000000ffff         /* 0x10: data */
    .quad 0x00209a000000ffff         /* 0x18: 64-bit code */
gdt_end:

.balign 8
gdtr:
    .word gdt_end - gdt - 1
    .quad gdt - smp_trampoline_start + {TRAMP_PHYS}

smp_trampoline_end:
"#,
    TRAMP_PHYS = const TRAMPOLINE_PHYSICAL,
    CR3_SLOT = const PARAM_CR3,
    RSP_SLOT = const PARAM_RSP,
    TARGET_SLOT = const PARAM_TARGET,
    CPU_ID_SLOT = const PARAM_CPU_ID,
    options(att_syntax)
);

unsafe extern "C" {
    fn smp_trampoline_start();
    fn smp_trampoline_end();
}

/// Bring up the application processors described by the ACPI MADT.
///
/// Silently stays single-core when no RSDP/MADT is available or when only
/// the BSP is present, so a `-smp 1` machine boots with byte-identical
/// output to before. A detected-but-failed bring-up logs exactly one line.
pub fn bring_up_application_processors(rsdp_address: Option<PhysicalAddress>) {
    // The RSDP address reaches arch code first through this call, so HPET
    // discovery piggybacks here rather than needing a new platform-side
    // wiring point. Emits exactly one `hpet:` line either way.
    crate::hpet::initialize(rsdp_address);

    let Some(lapic_ids) = acpi::enabled_lapic_ids(rsdp_address) else {
        return;
    };

    let bsp_apic_id = lapic::current_apic_id();
    let Some(bsp_position) = lapic_ids.iter().position(|&id| id == bsp_apic_id) else {
        return;
    };
    if bsp_position != 0 {
        serial::write_args(format_args!(
            "serviceos: smp: BSP not first in MADT; staying single-core\n"
        ));
        return;
    }

    let mut targets: Vec<(usize, u8)> = Vec::new();
    for (index, &apic_id) in lapic_ids.iter().enumerate() {
        if index == 0 || targets.len() >= MAX_SMP_CPUS - 1 {
            continue;
        }
        targets.push((index, apic_id));
    }
    if targets.is_empty() {
        return;
    }

    unsafe { install_trampoline() };
    APS_EXPECTED.store(targets.len() as u32, Ordering::SeqCst);

    let started = unsafe { start_application_processors(&targets) };
    let online = APS_ONLINE.load(Ordering::SeqCst);
    if !started || online < targets.len() as u32 {
        serial::write_args(format_args!(
            "serviceos: smp: AP bring-up incomplete ({}/{} online); staying single-core\n",
            online,
            targets.len()
        ));
    }
}

/// # Safety
/// Must run exactly once during boot before any SIPI is sent.
unsafe fn install_trampoline() {
    unsafe {
        let source = smp_trampoline_start as *const u8;
        let bytes = (smp_trampoline_end as *const () as usize)
            - (smp_trampoline_start as *const () as usize);
        assert!(bytes <= 512, "SMP trampoline outgrew its reserved window");
        core::ptr::copy_nonoverlapping(source, TRAMPOLINE_PHYSICAL as *mut u8, bytes);
    }
}

/// Run the INIT-SIPI sequence for each target sequentially, waiting for
/// each AP to reach [`ap_main`] before starting the next one so the shared
/// trampoline parameter block is never raced.
///
/// # Safety
/// The trampoline must already be installed, and the PIT must be running
/// since the delay windows busy-poll channel 0.
unsafe fn start_application_processors(targets: &[(usize, u8)]) -> bool {
    unsafe {
        core::ptr::write_volatile(
            PARAM_CR3 as *mut u64,
            crate::cpu::current_page_table_root().as_u64(),
        );
        core::ptr::write_volatile(PARAM_TARGET as *mut u64, ap_main as *const () as u64);
    }

    let init_delay_ticks = ticks_for_millis(INIT_DEASSERT_DELAY_MS);
    let timeout_ticks = ticks_for_millis(STARTUP_ONLINE_TIMEOUT_MS);

    for &(cpu_id, apic_id) in targets {
        let stack_top = &AP_STACKS[cpu_id].0 as *const u8 as u64 + AP_STACK_BYTES as u64;
        unsafe {
            core::ptr::write_volatile(PARAM_RSP as *mut u64, stack_top - 8);
            core::ptr::write_volatile(PARAM_CPU_ID as *mut u64, cpu_id as u64);
            lapic::send_init_ipi(apic_id);
        }
        (interrupts::external_irq_ops().wait_tick_wraps)(init_delay_ticks);
        unsafe {
            lapic::send_startup_ipi(apic_id, SIPI_VECTOR);
        }

        // Second SIPI is the standard retry for slow-to-start cores.
        if !wait_for_online(timeout_ticks / 2) {
            unsafe { lapic::send_startup_ipi(apic_id, SIPI_VECTOR) };
            if !wait_for_online(timeout_ticks / 2) {
                return false;
            }
        }
    }

    true
}

/// Poll the online counter once per reference tick until it reaches the
/// expected count or `max_ticks` periods elapse.
fn wait_for_online(max_ticks: u32) -> bool {
    #[allow(clippy::never_loop)]
    for _ in 0..max_ticks {
        (interrupts::external_irq_ops().wait_tick_wraps)(1);
        if APS_ONLINE.load(Ordering::SeqCst) == APS_EXPECTED.load(Ordering::SeqCst) {
            return true;
        }
    }
    APS_ONLINE.load(Ordering::SeqCst) == APS_EXPECTED.load(Ordering::SeqCst)
}

fn ticks_for_millis(millis: u32) -> u32 {
    ((interrupts::TIMER_TICK_HZ as u32 * millis) / 1000).max(1)
}

/// First C-ABI entry point on a newly-started application processor.
///
/// Reloads this CPU's descriptor-table registers from the shared BSP
/// tables, installs its own per-CPU data (GS base + privilege-stack top),
/// reports one serial line, and drops into the cooperative kernel-thread
/// idle loop with interrupts still disabled.
unsafe extern "C" fn ap_main(cpu_id: u64) -> ! {
    interrupts::initialize_ap(cpu_id as usize);
    APS_ONLINE.fetch_add(1, Ordering::SeqCst);
    serial::write_args(format_args!("serviceos: smp: ap{} online\n", cpu_id));
    kthread::ap_idle_loop(cpu_id as usize)
}
