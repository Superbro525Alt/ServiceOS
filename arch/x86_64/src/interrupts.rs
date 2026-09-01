mod faults;
mod irq;
mod syscall;
mod syscall_fast;

use core::arch::global_asm;

use serviceos_kernel_core::interrupts::InterruptVector;
use spin::Once;
use x86_64::{
    PrivilegeLevel, VirtAddr,
    instructions::{
        segmentation::{CS, DS, ES, SS, Segment},
        tables::load_tss,
    },
    structures::{
        gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector},
        idt::InterruptDescriptorTable,
        tss::TaskStateSegment,
    },
};

use crate::{cpu, lapic, msr};

global_asm!(include_str!("timer_irq_entry.S"));
global_asm!(include_str!("syscall_entry.S"));
global_asm!(include_str!("syscall_fast_entry.S"));

const DOUBLE_FAULT_IST_INDEX: u16 = 0;
const DOUBLE_FAULT_STACK_SIZE: usize = 16 * 1024;
const PRIVILEGE_STACK_INDEX: usize = 0;
const PRIVILEGE_STACK_SIZE: usize = 16 * 1024;
const EXTERNAL_IRQ_LINES: usize = 16;
const MAX_EXTERNAL_IRQ_HANDLERS_PER_LINE: usize = 4;

/// Vector where the platform's external IRQ lines land after bring-up. The
/// platform remap must place the 16 legacy lines at this base; secondary
/// controller offsets are platform-internal details.
pub const EXTERNAL_IRQ_VECTOR_BASE: u8 = 0x20;
pub const TIMER_VECTOR: u8 = EXTERNAL_IRQ_VECTOR_BASE;
pub const SYSCALL_VECTOR: u8 = 0x80;
pub const TIMER_TICK_HZ: u32 = 100;

/// Vector where message-signaled (MSI/MSI-X) device interrupts land. MSI
/// writes bypass the external IRQ controller entirely — the device DMA-writes
/// an address/data pair that the LAPIC turns into this vector — so the only
/// arch-side requirement is an IDT gate plus a handler table of our own.
/// 0x50 sits in priority class 5, below the LAPIC timer (0x40) and clear of
/// the external/PIC range (0x20-0x2F) and the syscall vector (0x80).
pub const MSI_VECTOR_BASE: u8 = 0x50;
/// How many message-signaled vectors the arch exposes. v0: the virtio NIC
/// (config + all queues share slot 0) and the virtio block device (slot 1),
/// one device-class vector each.
pub const MSI_VECTORS: usize = 2;

/// Operations the platform image provides for the external IRQ controller
/// and the reference tick source it programs (see
/// `serviceos-platform-x86-pc` for the PC implementation). The arch crate
/// orchestrates the ordering but owns none of the controller details.
pub struct ExternalInterruptOps {
    /// Remap and enable the external IRQ controller in its boot mode.
    pub bring_up: fn(),
    /// Program the reference tick source to interrupt at `hz`.
    pub program_tick_source: fn(hz: u32),
    /// Mask one external IRQ line (0-15).
    pub mask_line: fn(irq_line: u8),
    /// Unmask one external IRQ line (0-15).
    pub unmask_line: fn(irq_line: u8),
    /// Acknowledge (EOI) an external vector so further deliveries flow.
    pub acknowledge_vector: fn(vector: u8),
    /// Busy-wait until the reference tick source has wrapped `wraps` times;
    /// false on timeout (source not counting).
    pub wait_tick_wraps: fn(wraps: u32) -> bool,
}

static EXTERNAL_IRQ_OPS: Once<&'static ExternalInterruptOps> = Once::new();

/// The platform-provided external IRQ operations, installed by
/// [`initialize`]. Every caller runs during or after kernel bring-up.
pub fn external_irq_ops() -> &'static ExternalInterruptOps {
    EXTERNAL_IRQ_OPS
        .get()
        .expect("external IRQ ops must be installed before use")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DescriptorState {
    pub gdt_loaded: bool,
    pub idt_loaded: bool,
    pub tss_loaded: bool,
    pub external_controller_ready: bool,
    pub tick_source_programmed: bool,
    pub timer_hz: u32,
    pub syscall_vector: InterruptVector,
}

impl DescriptorState {
    pub const fn uninitialized() -> Self {
        Self {
            gdt_loaded: false,
            idt_loaded: false,
            tss_loaded: false,
            external_controller_ready: false,
            tick_source_programmed: false,
            timer_hz: 0,
            syscall_vector: InterruptVector(SYSCALL_VECTOR as u16),
        }
    }
}

#[repr(C, align(16))]
struct InterruptStack<const N: usize>([u8; N]);

#[derive(Clone, Copy)]
struct SegmentSelectors {
    kernel_code: SegmentSelector,
    kernel_data: SegmentSelector,
    user_code: SegmentSelector,
    user_data: SegmentSelector,
    tss: SegmentSelector,
}

struct DescriptorTables {
    gdt: GlobalDescriptorTable,
    selectors: SegmentSelectors,
}

static DOUBLE_FAULT_STACK: InterruptStack<DOUBLE_FAULT_STACK_SIZE> =
    InterruptStack([0; DOUBLE_FAULT_STACK_SIZE]);
static PRIVILEGE_STACK: InterruptStack<PRIVILEGE_STACK_SIZE> =
    InterruptStack([0; PRIVILEGE_STACK_SIZE]);
static TSS: Once<TaskStateSegment> = Once::new();
static DESCRIPTOR_TABLES: Once<DescriptorTables> = Once::new();
static IDT: Once<InterruptDescriptorTable> = Once::new();
static EXTERNAL_IRQ_HANDLERS: spin::Mutex<
    [[Option<fn(u8)>; MAX_EXTERNAL_IRQ_HANDLERS_PER_LINE]; EXTERNAL_IRQ_LINES],
> = spin::Mutex::new([[None; MAX_EXTERNAL_IRQ_HANDLERS_PER_LINE]; EXTERNAL_IRQ_LINES]);
static MSI_HANDLERS: spin::Mutex<[Option<fn(u8)>; MSI_VECTORS]> = spin::Mutex::new([None, None]);

unsafe extern "C" {
    fn serviceos_x86_64_syscall_entry();
    fn serviceos_x86_64_timer_irq_entry();
}

pub fn initialize(external: &'static ExternalInterruptOps) -> DescriptorState {
    install_descriptor_tables();
    install_interrupt_table();
    initialize_lapic();
    EXTERNAL_IRQ_OPS.call_once(|| external);
    (external.bring_up)();
    (external.program_tick_source)(TIMER_TICK_HZ);
    initialize_syscall_sysret();
    initialize_per_cpu();
    initialize_lapic_timer_source();
    serviceos_kernel_core::task::register_current_cpu_hook(current_gs_cpu_index);
    crate::kthread::spawn_pingpong_demo();
    // Drain the two-thread ping-pong smoke test synchronously: each round
    // crosses a real register-level context switch, and the final exit
    // prints one serial line proving switches occurred.
    crate::kthread::pump_pending();

    DescriptorState {
        gdt_loaded: true,
        idt_loaded: true,
        tss_loaded: true,
        external_controller_ready: true,
        tick_source_programmed: true,
        timer_hz: TIMER_TICK_HZ,
        syscall_vector: InterruptVector(SYSCALL_VECTOR as u16),
    }
}

fn initialize_lapic() {
    // Enable the local APIC as an interrupt controller in virtual-wire mode
    // so the platform's external controller keeps delivering and LAPIC EOIs
    // are meaningful. The platform tick source remains the system tick; the
    // LAPIC timer entry stays masked on its own vector until it is
    // calibrated against that reference.
    unsafe {
        lapic::initialize();
    }
}

/// Calibrate the LAPIC timer against the platform's running tick source and,
/// on success, arm it as the system tick source (masking the external IRQ
/// line 0 so ticks are counted exactly once). On failure the system silently
/// stays on the platform tick source.
fn initialize_lapic_timer_source() {
    const CALIBRATION_TICKS: u32 = 3;

    let mut timer = lapic::timer();
    let Some(ticks_per_ms) = timer.calibrate_against_reference(TIMER_TICK_HZ, CALIBRATION_TICKS)
    else {
        crate::serial::write_args(format_args!(
            "serviceos: lapic-timer: calibration failed; staying on PIT @{}Hz\n",
            TIMER_TICK_HZ
        ));
        return;
    };

    unsafe {
        timer.arm_periodic(TIMER_TICK_HZ, ticks_per_ms);
    }
    if !timer.is_armed() {
        crate::serial::write_args(format_args!(
            "serviceos: lapic-timer: arm failed; staying on PIT @{}Hz\n",
            TIMER_TICK_HZ
        ));
        return;
    }

    (external_irq_ops().mask_line)(0);
    crate::serial::write_args(format_args!(
        "serviceos: lapic-timer: calibrated {} ticks/ms; armed periodic on vector {:#04x}; PIT IRQ0 masked\n",
        ticks_per_ms,
        lapic::LAPIC_TIMER_VECTOR
    ));
}

fn initialize_syscall_sysret() {
    unsafe extern "C" {
        fn serviceos_x86_64_syscall_fast_entry();
    }

    let selectors = DESCRIPTOR_TABLES
        .get()
        .expect("descriptor tables initialized")
        .selectors;

    unsafe {
        msr::enable_syscall_sysret(
            serviceos_x86_64_syscall_fast_entry as *const () as u64,
            selectors.kernel_code.0,
            selectors.user_code.0,
            selectors.user_data.0,
        );
    }
}

fn initialize_per_cpu() {
    // Point GS base at CPU 0's PerCpuData with the privilege (kernel) stack
    // top as the fast-syscall kernel RSP. The fast path stub reads/writes
    // gs:[0x00] (kernel_rsp) and gs:[0x08] (user_rsp), matching the
    // PerCpuData field layout, and never swaps GS, so the base must be valid
    // from this point on in every context.
    let privilege_stack_top =
        VirtAddr::from_ptr(&PRIVILEGE_STACK.0).as_u64() + PRIVILEGE_STACK_SIZE as u64;
    unsafe {
        crate::per_cpu::initialize_per_cpu_data(0, privilege_stack_top);
    }
}

fn current_gs_cpu_index() -> usize {
    // SAFETY: GS base points at this CPU's PerCpuData everywhere this hook
    // can be invoked (it is registered only after initialize_per_cpu).
    unsafe { crate::per_cpu::current_cpu_data().cpu_id as usize }
}

/// Re-arm this CPU's descriptor tables and per-CPU data after an AP enters
/// long mode via the SMP trampoline.
///
/// The GDT/IDT/TSS themselves are shared, `Once`-initialized structures from
/// the BSP boot; each CPU still needs its own LGDT/LIDT/LTR because those
/// registers are per-CPU reset state. Interrupts stay disabled on APs after
/// this call — they never enable them, so no per-CPU LAPIC timer or syscall
/// MSR setup is required yet.
pub fn initialize_ap(cpu_id: usize) {
    let descriptor_tables = DESCRIPTOR_TABLES
        .get()
        .expect("BSP descriptor tables must be initialized before AP bring-up");
    descriptor_tables.gdt.load();

    unsafe {
        CS::set_reg(descriptor_tables.selectors.kernel_code);
        SS::set_reg(descriptor_tables.selectors.kernel_data);
        DS::set_reg(descriptor_tables.selectors.kernel_data);
        ES::set_reg(descriptor_tables.selectors.kernel_data);
    }
    // Deliberately NO load_tss here: the shared GDT's TSS descriptor went
    // BUSY when the BSP loaded it, and an LTR against a busy descriptor
    // faults. APs currently stay ring-0 with interrupts disabled (they run
    // the cooperative kernel-thread idle loop), so task-register state is
    // never consulted until per-CPU TSS support lands.

    let idt = IDT
        .get()
        .expect("BSP IDT must be initialized before AP bring-up");
    idt.load();

    let privilege_stack_top =
        VirtAddr::from_ptr(&PRIVILEGE_STACK.0).as_u64() + PRIVILEGE_STACK_SIZE as u64;
    unsafe {
        crate::per_cpu::initialize_per_cpu_data(cpu_id, privilege_stack_top);
    }
}

pub fn arm_demo_wakeup(deadline_ticks_from_now: u64) {
    cpu::with_interrupts_disabled(|| {
        let Some(manager) = serviceos_kernel_core::time::manager() else {
            return;
        };

        let deadline = manager.now().saturating_add(deadline_ticks_from_now);
        let _ = serviceos_kernel_core::time::TimerService::arm_wakeup(
            manager,
            serviceos_kernel_core::time::WakeToken(1),
            serviceos_kernel_core::time::TimerRequest::one_shot(deadline),
        );
    });
}

pub fn poll_wakeup() -> Option<serviceos_kernel_core::time::WakeEvent> {
    cpu::with_interrupts_disabled(|| {
        serviceos_kernel_core::time::manager().and_then(|manager| manager.take_wakeup())
    })
}

pub fn register_external_irq_handler(irq_line: u8, handler: fn(u8)) -> bool {
    irq::register_external_irq_handler(irq_line, handler)
}

/// Register a handler for one message-signaled (MSI/MSI-X) vector slot.
///
/// Unlike external IRQ lines these vectors never pass through the platform's
/// external controller: the IDT gate acknowledges via LAPIC EOI only. The
/// caller still programs the device's message table (address/data/vector
/// control) through PCI config/MMIO — this only wires the CPU side.
pub fn register_msi_vector_handler(vector_slot: u8, handler: fn(u8)) -> bool {
    if usize::from(vector_slot) >= MSI_VECTORS {
        return false;
    }

    let mut handlers = MSI_HANDLERS.lock();
    for existing in handlers[usize::from(vector_slot)].iter().copied() {
        if core::ptr::fn_addr_eq(existing, handler) {
            return true;
        }
    }
    if handlers[usize::from(vector_slot)].is_none() {
        handlers[usize::from(vector_slot)] = Some(handler);
        return true;
    }
    false
}

pub(crate) fn dispatch_msi_vector(slot: u8) {
    let vector = MSI_VECTOR_BASE + slot;
    serviceos_kernel_core::interrupts::note_external_interrupt(InterruptVector(vector as u16));
    let handler = { MSI_HANDLERS.lock()[usize::from(slot)] };
    if let Some(handler) = handler {
        handler(slot);
    }
    // MSI deliveries arrive through the LAPIC, so EOI there. Unlike the
    // external path there is no external controller to acknowledge.
    unsafe {
        if crate::lapic::timer().is_initialized() {
            crate::lapic::send_eoi();
        }
    }
}

pub(crate) fn user_code_selector() -> SegmentSelector {
    let selector = DESCRIPTOR_TABLES
        .get()
        .expect("descriptor tables initialized")
        .selectors
        .user_code;

    SegmentSelector(selector.0 | 0b11)
}

pub(crate) fn user_data_selector() -> SegmentSelector {
    let selector = DESCRIPTOR_TABLES
        .get()
        .expect("descriptor tables initialized")
        .selectors
        .user_data;

    SegmentSelector(selector.0 | 0b11)
}

fn install_descriptor_tables() {
    let descriptor_tables = DESCRIPTOR_TABLES.call_once(|| {
        let mut gdt = GlobalDescriptorTable::new();
        let kernel_code = gdt.append(Descriptor::kernel_code_segment());
        // SYSCALL loads SS as (kernel CS) + 8 and SYSRET derives SS as
        // (user CS | 3) + 8, so each data segment must directly follow its
        // matching code segment.
        let kernel_data = gdt.append(Descriptor::kernel_data_segment());
        let user_code = gdt.append(Descriptor::user_code_segment());
        let user_data = gdt.append(Descriptor::user_data_segment());
        let tss = gdt.append(Descriptor::tss_segment(tss()));

        DescriptorTables {
            gdt,
            selectors: SegmentSelectors {
                kernel_code,
                kernel_data,
                user_code,
                user_data,
                tss,
            },
        }
    });

    descriptor_tables.gdt.load();

    unsafe {
        CS::set_reg(descriptor_tables.selectors.kernel_code);
        SS::set_reg(descriptor_tables.selectors.kernel_data);
        DS::set_reg(descriptor_tables.selectors.kernel_data);
        ES::set_reg(descriptor_tables.selectors.kernel_data);
        load_tss(descriptor_tables.selectors.tss);
    }
}

fn install_interrupt_table() {
    let idt = IDT.call_once(|| {
        let mut idt = InterruptDescriptorTable::new();

        idt.divide_error
            .set_handler_fn(faults::divide_error_handler);
        idt.debug.set_handler_fn(faults::debug_exception_handler);
        idt.non_maskable_interrupt
            .set_handler_fn(faults::non_maskable_interrupt_handler);
        idt.breakpoint.set_handler_fn(faults::breakpoint_handler);
        idt.overflow.set_handler_fn(faults::overflow_handler);
        idt.bound_range_exceeded
            .set_handler_fn(faults::bound_range_exceeded_handler);
        idt.invalid_opcode
            .set_handler_fn(faults::invalid_opcode_handler);
        idt.device_not_available
            .set_handler_fn(faults::device_not_available_handler);
        unsafe {
            idt.double_fault
                .set_handler_fn(faults::double_fault_handler)
                .set_stack_index(DOUBLE_FAULT_IST_INDEX);
        }
        idt.invalid_tss.set_handler_fn(faults::invalid_tss_handler);
        idt.segment_not_present
            .set_handler_fn(faults::segment_not_present_handler);
        idt.stack_segment_fault
            .set_handler_fn(faults::stack_segment_fault_handler);
        idt.general_protection_fault
            .set_handler_fn(faults::general_protection_fault_handler);
        idt.page_fault.set_handler_fn(faults::page_fault_handler);
        idt.x87_floating_point
            .set_handler_fn(faults::x87_floating_point_handler);
        idt.alignment_check
            .set_handler_fn(faults::alignment_check_handler);
        idt.machine_check
            .set_handler_fn(faults::machine_check_handler);
        idt.simd_floating_point
            .set_handler_fn(faults::simd_floating_point_handler);
        idt.virtualization
            .set_handler_fn(faults::virtualization_handler);
        idt.cp_protection_exception
            .set_handler_fn(faults::control_protection_handler);
        idt.hv_injection_exception
            .set_handler_fn(faults::hypervisor_injection_handler);
        idt.vmm_communication_exception
            .set_handler_fn(faults::vmm_communication_handler);
        idt.security_exception
            .set_handler_fn(faults::security_exception_handler);
        unsafe {
            idt[TIMER_VECTOR].set_handler_addr(VirtAddr::from_ptr(
                serviceos_x86_64_timer_irq_entry as *const (),
            ));
            // The LAPIC timer shares the timer IRQ entry stub; it fires on
            // its own vector once armed and the Rust body picks the correct
            // controller acknowledgement from the armed flag.
            idt[lapic::LAPIC_TIMER_VECTOR].set_handler_addr(VirtAddr::from_ptr(
                serviceos_x86_64_timer_irq_entry as *const (),
            ));
            idt[lapic::LAPIC_SPURIOUS_VECTOR].set_handler_fn(irq::lapic_spurious_interrupt_handler);
            idt[MSI_VECTOR_BASE].set_handler_fn(irq::msi_vector_handler);
            idt[MSI_VECTOR_BASE + 1].set_handler_fn(irq::msi_vector_handler_1);
            idt[EXTERNAL_IRQ_VECTOR_BASE + 1].set_handler_fn(irq::external_irq1_handler);
            idt[EXTERNAL_IRQ_VECTOR_BASE + 2].set_handler_fn(irq::external_irq2_handler);
            idt[EXTERNAL_IRQ_VECTOR_BASE + 3].set_handler_fn(irq::external_irq3_handler);
            idt[EXTERNAL_IRQ_VECTOR_BASE + 4].set_handler_fn(irq::external_irq4_handler);
            idt[EXTERNAL_IRQ_VECTOR_BASE + 5].set_handler_fn(irq::external_irq5_handler);
            idt[EXTERNAL_IRQ_VECTOR_BASE + 6].set_handler_fn(irq::external_irq6_handler);
            idt[EXTERNAL_IRQ_VECTOR_BASE + 7].set_handler_fn(irq::external_irq7_handler);
            idt[EXTERNAL_IRQ_VECTOR_BASE + 8].set_handler_fn(irq::external_irq8_handler);
            idt[EXTERNAL_IRQ_VECTOR_BASE + 9].set_handler_fn(irq::external_irq9_handler);
            idt[EXTERNAL_IRQ_VECTOR_BASE + 10].set_handler_fn(irq::external_irq10_handler);
            idt[EXTERNAL_IRQ_VECTOR_BASE + 11].set_handler_fn(irq::external_irq11_handler);
            idt[EXTERNAL_IRQ_VECTOR_BASE + 12].set_handler_fn(irq::external_irq12_handler);
            idt[EXTERNAL_IRQ_VECTOR_BASE + 13].set_handler_fn(irq::external_irq13_handler);
            idt[EXTERNAL_IRQ_VECTOR_BASE + 14].set_handler_fn(irq::external_irq14_handler);
            idt[EXTERNAL_IRQ_VECTOR_BASE + 15].set_handler_fn(irq::external_irq15_handler);
            idt[SYSCALL_VECTOR]
                .set_handler_addr(VirtAddr::from_ptr(
                    serviceos_x86_64_syscall_entry as *const (),
                ))
                .set_privilege_level(PrivilegeLevel::Ring3);
        }

        idt
    });

    idt.load();
}

fn tss() -> &'static TaskStateSegment {
    TSS.call_once(|| {
        let mut tss = TaskStateSegment::new();
        let stack_base = VirtAddr::from_ptr(&DOUBLE_FAULT_STACK.0);
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] =
            stack_base + DOUBLE_FAULT_STACK_SIZE as u64;
        let privilege_stack_base = VirtAddr::from_ptr(&PRIVILEGE_STACK.0);
        tss.privilege_stack_table[PRIVILEGE_STACK_INDEX] =
            privilege_stack_base + PRIVILEGE_STACK_SIZE as u64;
        tss
    })
}
