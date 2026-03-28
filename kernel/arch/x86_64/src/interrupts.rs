use core::arch::global_asm;

use serviceos_kernel_core::{
    interrupts::{self, ExceptionDetail, ExceptionReport, InterruptVector, TrapFrameView},
    syscall::{SyscallContext, SyscallNumber},
    task,
};
use spin::Once;
use x86_64::{
    PrivilegeLevel, VirtAddr,
    instructions::{
        port::Port,
        segmentation::{CS, DS, ES, SS, Segment},
        tables::load_tss,
    },
    structures::{
        gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector},
        idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode},
        tss::TaskStateSegment,
    },
};

use crate::{cpu, serial, user::SavedUserContext};

global_asm!(include_str!("syscall_entry.S"));

const PIC1_COMMAND_PORT: u16 = 0x20;
const PIC1_DATA_PORT: u16 = 0x21;
const PIC2_COMMAND_PORT: u16 = 0xA0;
const PIC2_DATA_PORT: u16 = 0xA1;
const PIC_EOI: u8 = 0x20;
const PIC_ICW1_INIT: u8 = 0x11;
const PIC_ICW4_8086: u8 = 0x01;
const PIT_COMMAND_PORT: u16 = 0x43;
const PIT_CHANNEL0_PORT: u16 = 0x40;
const PIT_INPUT_HZ: u32 = 1_193_182;
const DOUBLE_FAULT_IST_INDEX: u16 = 0;
const DOUBLE_FAULT_STACK_SIZE: usize = 16 * 1024;
const PRIVILEGE_STACK_INDEX: usize = 0;
const PRIVILEGE_STACK_SIZE: usize = 16 * 1024;

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

unsafe extern "C" {
    fn serviceos_x86_64_syscall_entry();
}

pub fn initialize() -> DescriptorState {
    install_descriptor_tables();
    install_interrupt_table();
    initialize_pic();
    initialize_pit(TIMER_TICK_HZ);

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

        idt.divide_error.set_handler_fn(divide_error_handler);
        idt.debug.set_handler_fn(debug_exception_handler);
        idt.non_maskable_interrupt
            .set_handler_fn(non_maskable_interrupt_handler);
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        idt.overflow.set_handler_fn(overflow_handler);
        idt.bound_range_exceeded
            .set_handler_fn(bound_range_exceeded_handler);
        idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
        idt.device_not_available
            .set_handler_fn(device_not_available_handler);
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(DOUBLE_FAULT_IST_INDEX);
        }
        idt.invalid_tss.set_handler_fn(invalid_tss_handler);
        idt.segment_not_present
            .set_handler_fn(segment_not_present_handler);
        idt.stack_segment_fault
            .set_handler_fn(stack_segment_fault_handler);
        idt.general_protection_fault
            .set_handler_fn(general_protection_fault_handler);
        idt.page_fault.set_handler_fn(page_fault_handler);
        idt.x87_floating_point
            .set_handler_fn(x87_floating_point_handler);
        idt.alignment_check.set_handler_fn(alignment_check_handler);
        idt.machine_check.set_handler_fn(machine_check_handler);
        idt.simd_floating_point
            .set_handler_fn(simd_floating_point_handler);
        idt.virtualization.set_handler_fn(virtualization_handler);
        idt.cp_protection_exception
            .set_handler_fn(control_protection_handler);
        idt.hv_injection_exception
            .set_handler_fn(hypervisor_injection_handler);
        idt.vmm_communication_exception
            .set_handler_fn(vmm_communication_handler);
        idt.security_exception
            .set_handler_fn(security_exception_handler);
        unsafe {
            idt[TIMER_VECTOR].set_handler_fn(timer_interrupt_handler);
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

fn initialize_pic() {
    let mut pic1_command = Port::<u8>::new(PIC1_COMMAND_PORT);
    let mut pic1_data = Port::<u8>::new(PIC1_DATA_PORT);
    let mut pic2_command = Port::<u8>::new(PIC2_COMMAND_PORT);
    let mut pic2_data = Port::<u8>::new(PIC2_DATA_PORT);

    unsafe {
        let _saved_mask1 = pic1_data.read();
        let _saved_mask2 = pic2_data.read();

        pic1_command.write(PIC_ICW1_INIT);
        io_wait();
        pic2_command.write(PIC_ICW1_INIT);
        io_wait();

        pic1_data.write(PIC_PRIMARY_OFFSET);
        io_wait();
        pic2_data.write(PIC_SECONDARY_OFFSET);
        io_wait();

        pic1_data.write(0x04);
        io_wait();
        pic2_data.write(0x02);
        io_wait();

        pic1_data.write(PIC_ICW4_8086);
        io_wait();
        pic2_data.write(PIC_ICW4_8086);
        io_wait();

        pic1_data.write(0b1111_1110);
        pic2_data.write(0b1111_1111);
    }
}

fn initialize_pit(tick_hz: u32) {
    let divisor = (PIT_INPUT_HZ / tick_hz.max(1)) as u16;
    let [low, high] = divisor.to_le_bytes();
    let mut pit_command = Port::<u8>::new(PIT_COMMAND_PORT);
    let mut pit_channel0 = Port::<u8>::new(PIT_CHANNEL0_PORT);

    unsafe {
        pit_command.write(0x36);
        pit_channel0.write(low);
        pit_channel0.write(high);
    }
}

fn io_wait() {
    let mut wait_port = Port::<u8>::new(0x80);

    unsafe {
        wait_port.write(0);
    }
}

fn acknowledge_pic(vector: u8) {
    let mut pic1_command = Port::<u8>::new(PIC1_COMMAND_PORT);
    let mut pic2_command = Port::<u8>::new(PIC2_COMMAND_PORT);

    unsafe {
        if vector >= PIC_SECONDARY_OFFSET {
            pic2_command.write(PIC_EOI);
        }

        pic1_command.write(PIC_EOI);
    }
}

fn frame_view(frame: &InterruptStackFrame) -> TrapFrameView {
    TrapFrameView {
        instruction_pointer: frame.instruction_pointer.as_u64(),
        stack_pointer: frame.stack_pointer.as_u64(),
        flags: frame.cpu_flags.bits(),
        code_segment: frame.code_segment.0 as u64,
    }
}

fn handle_exception(report: ExceptionReport) -> ! {
    log_exception(report);

    if matches!(
        report.disposition,
        serviceos_kernel_core::interrupts::FaultDisposition::TerminateTask
    ) {
        terminate_faulting_user_task(report);
    }

    cpu::halt_loop()
}

fn terminate_faulting_user_task(report: ExceptionReport) -> ! {
    serial::write_args(format_args!(
        "serviceos: interrupt: terminating faulting userspace task exit={:#x}\n",
        user_fault_exit_code(report)
    ));
    serviceos_kernel_core::user::mark_current_thread_exited(user_fault_exit_code(report));
    if let Some(tasks) = task::system() {
        let _ = tasks.scheduler().terminate_current();
    }
    crate::user::return_to_kernel()
}

fn user_fault_exit_code(report: ExceptionReport) -> u64 {
    const USER_FAULT_EXIT_TAG: u64 = 0xf100_0000_0000_0000;

    let detail = match report.detail {
        ExceptionDetail::InvalidOpcode => 6,
        ExceptionDetail::PageFault { error_code, .. } => 0x100 | (error_code & 0xff),
        ExceptionDetail::GeneralProtection { error_code } => 0x200 | (error_code & 0xff),
        ExceptionDetail::Unknown { vector, .. } => 0x300 | vector.0 as u64,
        ExceptionDetail::DoubleFault { error_code } => 0x400 | (error_code & 0xff),
        ExceptionDetail::Breakpoint => 3,
    };

    USER_FAULT_EXIT_TAG | detail
}

fn log_exception(report: ExceptionReport) {
    match report.detail {
        ExceptionDetail::Breakpoint => {
            serial::write_args(format_args!(
                "serviceos: breakpoint trap at ip={:#x}\n",
                report.frame.instruction_pointer
            ));
        }
        ExceptionDetail::InvalidOpcode => {
            serial::write_args(format_args!(
                "serviceos: invalid opcode at ip={:#x} origin={:?}\n",
                report.frame.instruction_pointer,
                report.frame.origin()
            ));
        }
        ExceptionDetail::DoubleFault { error_code } => {
            serial::write_args(format_args!(
                "serviceos: double fault error={:#x} ip={:#x}\n",
                error_code, report.frame.instruction_pointer
            ));
        }
        ExceptionDetail::GeneralProtection { error_code } => {
            serial::write_args(format_args!(
                "serviceos: general protection fault error={:#x} ip={:#x} origin={:?}\n",
                error_code,
                report.frame.instruction_pointer,
                report.frame.origin()
            ));
        }
        ExceptionDetail::PageFault {
            fault_address,
            error_code,
        } => {
            serial::write_args(format_args!(
                "serviceos: page fault addr={:#x} error={:#x} ip={:#x} origin={:?}\n",
                fault_address,
                error_code,
                report.frame.instruction_pointer,
                report.frame.origin()
            ));
        }
        ExceptionDetail::Unknown { vector, error_code } => {
            serial::write_args(format_args!(
                "serviceos: exception vector={} error={:?} ip={:#x} origin={:?}\n",
                vector.0,
                error_code,
                report.frame.instruction_pointer,
                report.frame.origin()
            ));
        }
    }
}

fn fatal_unknown_exception(frame: InterruptStackFrame, vector: u8, error_code: Option<u64>) -> ! {
    handle_exception(interrupts::handle_exception(
        ExceptionDetail::Unknown {
            vector: serviceos_kernel_core::interrupts::ExceptionVector(vector),
            error_code,
        },
        frame_view(&frame),
    ))
}

extern "x86-interrupt" fn timer_interrupt_handler(_frame: InterruptStackFrame) {
    let _ = interrupts::note_timer_interrupt(InterruptVector(TIMER_VECTOR as u16));
    crate::network::poll_ready_interfaces();
    crate::input::poll_ready_sources();
    acknowledge_pic(TIMER_VECTOR);
}

extern "x86-interrupt" fn breakpoint_handler(frame: InterruptStackFrame) {
    let report = interrupts::handle_exception(ExceptionDetail::Breakpoint, frame_view(&frame));
    if matches!(
        report.disposition,
        serviceos_kernel_core::interrupts::FaultDisposition::Fatal
    ) {
        handle_exception(report);
    }
}

extern "x86-interrupt" fn divide_error_handler(frame: InterruptStackFrame) {
    fatal_unknown_exception(frame, 0, None);
}

extern "x86-interrupt" fn debug_exception_handler(frame: InterruptStackFrame) {
    fatal_unknown_exception(frame, 1, None);
}

extern "x86-interrupt" fn non_maskable_interrupt_handler(frame: InterruptStackFrame) {
    fatal_unknown_exception(frame, 2, None);
}

extern "x86-interrupt" fn overflow_handler(frame: InterruptStackFrame) {
    fatal_unknown_exception(frame, 4, None);
}

extern "x86-interrupt" fn bound_range_exceeded_handler(frame: InterruptStackFrame) {
    fatal_unknown_exception(frame, 5, None);
}

extern "x86-interrupt" fn invalid_opcode_handler(frame: InterruptStackFrame) {
    handle_exception(interrupts::handle_exception(
        ExceptionDetail::InvalidOpcode,
        frame_view(&frame),
    ));
}

extern "x86-interrupt" fn device_not_available_handler(frame: InterruptStackFrame) {
    fatal_unknown_exception(frame, 7, None);
}

extern "x86-interrupt" fn double_fault_handler(frame: InterruptStackFrame, error_code: u64) -> ! {
    handle_exception(interrupts::handle_exception(
        ExceptionDetail::DoubleFault { error_code },
        frame_view(&frame),
    ));
}

extern "x86-interrupt" fn invalid_tss_handler(frame: InterruptStackFrame, error_code: u64) {
    fatal_unknown_exception(frame, 10, Some(error_code));
}

extern "x86-interrupt" fn segment_not_present_handler(frame: InterruptStackFrame, error_code: u64) {
    fatal_unknown_exception(frame, 11, Some(error_code));
}

extern "x86-interrupt" fn stack_segment_fault_handler(frame: InterruptStackFrame, error_code: u64) {
    fatal_unknown_exception(frame, 12, Some(error_code));
}

extern "x86-interrupt" fn general_protection_fault_handler(
    frame: InterruptStackFrame,
    error_code: u64,
) {
    handle_exception(interrupts::handle_exception(
        ExceptionDetail::GeneralProtection { error_code },
        frame_view(&frame),
    ));
}

extern "x86-interrupt" fn page_fault_handler(
    frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    handle_exception(interrupts::handle_exception(
        ExceptionDetail::PageFault {
            fault_address: cpu::read_page_fault_address(),
            error_code: error_code.bits(),
        },
        frame_view(&frame),
    ));
}

extern "x86-interrupt" fn x87_floating_point_handler(frame: InterruptStackFrame) {
    fatal_unknown_exception(frame, 16, None);
}

extern "x86-interrupt" fn alignment_check_handler(frame: InterruptStackFrame, error_code: u64) {
    fatal_unknown_exception(frame, 17, Some(error_code));
}

extern "x86-interrupt" fn machine_check_handler(frame: InterruptStackFrame) -> ! {
    fatal_unknown_exception(frame, 18, None)
}

extern "x86-interrupt" fn simd_floating_point_handler(frame: InterruptStackFrame) {
    fatal_unknown_exception(frame, 19, None);
}

extern "x86-interrupt" fn virtualization_handler(frame: InterruptStackFrame) {
    fatal_unknown_exception(frame, 20, None);
}

extern "x86-interrupt" fn control_protection_handler(frame: InterruptStackFrame, error_code: u64) {
    fatal_unknown_exception(frame, 21, Some(error_code));
}

extern "x86-interrupt" fn hypervisor_injection_handler(frame: InterruptStackFrame) {
    fatal_unknown_exception(frame, 28, None);
}

extern "x86-interrupt" fn vmm_communication_handler(frame: InterruptStackFrame, error_code: u64) {
    fatal_unknown_exception(frame, 29, Some(error_code));
}

extern "x86-interrupt" fn security_exception_handler(frame: InterruptStackFrame, error_code: u64) {
    fatal_unknown_exception(frame, 30, Some(error_code));
}

#[unsafe(no_mangle)]
extern "C" fn serviceos_x86_64_handle_syscall(frame: &mut SavedUserContext) -> u64 {
    let context = SyscallContext {
        instruction_pointer: frame.instruction_pointer,
        stack_pointer: frame.user_stack_pointer,
        flags: frame.cpu_flags,
        arguments: [
            frame.rdi, frame.rsi, frame.rdx, frame.r10, frame.r8, frame.r9,
        ],
    };
    let result = interrupts::dispatch_syscall(SyscallNumber(frame.rax as u32), &context);

    frame.rax = result.value;
    frame.rdx = result.abi_error_code();
    match result.action {
        serviceos_kernel_core::syscall::SyscallAction::ReturnToCaller => 0,
        serviceos_kernel_core::syscall::SyscallAction::YieldCurrentThread => {
            if let Some(tasks) = serviceos_kernel_core::task::system() {
                if let Some(thread_id) = tasks.scheduler().current_thread() {
                    crate::user::save_thread_context(thread_id, frame);
                }
                let _ = tasks.scheduler().yield_current();
            }
            1
        }
        serviceos_kernel_core::syscall::SyscallAction::BlockCurrentThreadOnReceive { endpoint } => {
            if let Some(tasks) = serviceos_kernel_core::task::system() {
                if let Some(thread_id) = tasks.scheduler().current_thread() {
                    crate::user::save_thread_context(thread_id, frame);
                }
                let _ = tasks.scheduler().block_current_on_receive(endpoint);
            }
            1
        }
        serviceos_kernel_core::syscall::SyscallAction::BlockCurrentThreadOnPacketReceive {
            interface,
        } => {
            if let Some(tasks) = serviceos_kernel_core::task::system() {
                if let Some(thread_id) = tasks.scheduler().current_thread() {
                    crate::user::save_thread_context(thread_id, frame);
                }
                let _ = tasks.scheduler().block_current_on_packet_receive(interface);
            }
            1
        }
        serviceos_kernel_core::syscall::SyscallAction::BlockCurrentThreadOnInputReceive {
            source,
        } => {
            if let Some(tasks) = serviceos_kernel_core::task::system() {
                if let Some(thread_id) = tasks.scheduler().current_thread() {
                    crate::user::save_thread_context(thread_id, frame);
                }
                let _ = tasks.scheduler().block_current_on_input_receive(source);
            }
            1
        }
        serviceos_kernel_core::syscall::SyscallAction::ExitCurrentThread { status } => {
            serviceos_kernel_core::user::mark_current_thread_exited(status);
            if let Some(tasks) = serviceos_kernel_core::task::system() {
                let _ = tasks.scheduler().terminate_current();
            }
            1
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
