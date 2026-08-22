mod faults;
mod irq;
mod pic;
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

global_asm!(include_str!("syscall_entry.S"));
global_asm!(include_str!("syscall_fast_entry.S"));

const DOUBLE_FAULT_IST_INDEX: u16 = 0;
const DOUBLE_FAULT_STACK_SIZE: usize = 16 * 1024;
const PRIVILEGE_STACK_INDEX: usize = 0;
const PRIVILEGE_STACK_SIZE: usize = 16 * 1024;
const EXTERNAL_IRQ_LINES: usize = 16;
const MAX_EXTERNAL_IRQ_HANDLERS_PER_LINE: usize = 4;

pub const PIC_PRIMARY_OFFSET: u8 = 0x20;
pub const PIC_SECONDARY_OFFSET: u8 = 0x28;
pub const TIMER_VECTOR: u8 = PIC_PRIMARY_OFFSET;
pub const SYSCALL_VECTOR: u8 = 0x80;
pub const TIMER_TICK_HZ: u32 = 100;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DescriptorState {
    pub gdt_loaded: bool,
    pub idt_loaded: bool,
    pub tss_loaded: bool,
    pub pic_remapped: bool,
    pub pit_programmed: bool,
    pub timer_hz: u32,
    pub syscall_vector: InterruptVector,
}

impl DescriptorState {
    pub const fn uninitialized() -> Self {
        Self {
            gdt_loaded: false,
            idt_loaded: false,
            tss_loaded: false,
            pic_remapped: false,
            pit_programmed: false,
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

unsafe extern "C" {
    fn serviceos_x86_64_syscall_entry();
}

pub fn initialize() -> DescriptorState {
    install_descriptor_tables();
    install_interrupt_table();
    initialize_lapic();
    pic::initialize_pic();
    pic::initialize_pit(TIMER_TICK_HZ);
    initialize_syscall_sysret();

    DescriptorState {
        gdt_loaded: true,
        idt_loaded: true,
        tss_loaded: true,
        pic_remapped: true,
        pit_programmed: true,
        timer_hz: TIMER_TICK_HZ,
        syscall_vector: InterruptVector(SYSCALL_VECTOR as u16),
    }
}

fn initialize_lapic() {
    // Enable the local APIC as an interrupt controller in virtual-wire mode
    // so the PIC keeps delivering and LAPIC EOIs are meaningful. The PIT/PIC
    // remains the system tick source; the LAPIC timer entry stays masked on
    // its own vector until it is calibrated against the PIT.
    unsafe {
        lapic::initialize();
    }
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
        let kernel_data = gdt.append(Descriptor::kernel_data_segment());
        let user_data = gdt.append(Descriptor::user_data_segment());
        let user_code = gdt.append(Descriptor::user_code_segment());
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
            idt[TIMER_VECTOR].set_handler_fn(irq::timer_interrupt_handler);
            idt[lapic::LAPIC_SPURIOUS_VECTOR].set_handler_fn(irq::lapic_spurious_interrupt_handler);
            idt[PIC_PRIMARY_OFFSET + 1].set_handler_fn(irq::external_irq1_handler);
            idt[PIC_PRIMARY_OFFSET + 2].set_handler_fn(irq::external_irq2_handler);
            idt[PIC_PRIMARY_OFFSET + 3].set_handler_fn(irq::external_irq3_handler);
            idt[PIC_PRIMARY_OFFSET + 4].set_handler_fn(irq::external_irq4_handler);
            idt[PIC_PRIMARY_OFFSET + 5].set_handler_fn(irq::external_irq5_handler);
            idt[PIC_PRIMARY_OFFSET + 6].set_handler_fn(irq::external_irq6_handler);
            idt[PIC_PRIMARY_OFFSET + 7].set_handler_fn(irq::external_irq7_handler);
            idt[PIC_SECONDARY_OFFSET].set_handler_fn(irq::external_irq8_handler);
            idt[PIC_SECONDARY_OFFSET + 1].set_handler_fn(irq::external_irq9_handler);
            idt[PIC_SECONDARY_OFFSET + 2].set_handler_fn(irq::external_irq10_handler);
            idt[PIC_SECONDARY_OFFSET + 3].set_handler_fn(irq::external_irq11_handler);
            idt[PIC_SECONDARY_OFFSET + 4].set_handler_fn(irq::external_irq12_handler);
            idt[PIC_SECONDARY_OFFSET + 5].set_handler_fn(irq::external_irq13_handler);
            idt[PIC_SECONDARY_OFFSET + 6].set_handler_fn(irq::external_irq14_handler);
            idt[PIC_SECONDARY_OFFSET + 7].set_handler_fn(irq::external_irq15_handler);
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
