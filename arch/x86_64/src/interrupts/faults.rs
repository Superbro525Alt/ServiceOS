use serviceos_kernel_core::{
    interrupts::{self, ExceptionDetail, ExceptionReport, TrapFrameView},
    task,
};
use x86_64::structures::idt::{InterruptStackFrame, PageFaultErrorCode};

use crate::{cpu, serial};

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
        let fault_type = serviceos_kernel_core::fault::fault_type_for_exception(&report.detail);
        if let Some(handler) = serviceos_kernel_core::fault::lookup_fault_handler(&fault_type) {
            let endpoint = handler.endpoint;
            
            serial::write_args(format_args!(
                "serviceos: fault handler found for type={:?}, notifying endpoint\n",
                fault_type
            ));
            
            if let Some(tasks) = task::system() {
                tasks.notify_object_ready(endpoint);
            }
            
            crate::user::return_to_kernel();
        } else {
            terminate_faulting_user_task(report);
        }
    }

    cpu::halt_loop()
}

fn terminate_faulting_user_task(report: ExceptionReport) -> ! {
    serial::write_args(format_args!(
        "serviceos: interrupt: terminating faulting userspace task exit={:#x}\n",
        user_fault_exit_code(report)
    ));
    serviceos_kernel_core::user::mark_current_thread_faulted(user_fault_exit_code(report));
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

pub(super) extern "x86-interrupt" fn breakpoint_handler(frame: InterruptStackFrame) {
    let report = interrupts::handle_exception(ExceptionDetail::Breakpoint, frame_view(&frame));
    if matches!(
        report.disposition,
        serviceos_kernel_core::interrupts::FaultDisposition::Fatal
    ) {
        handle_exception(report);
    }
}

pub(super) extern "x86-interrupt" fn divide_error_handler(frame: InterruptStackFrame) {
    fatal_unknown_exception(frame, 0, None);
}

pub(super) extern "x86-interrupt" fn debug_exception_handler(frame: InterruptStackFrame) {
    fatal_unknown_exception(frame, 1, None);
}

pub(super) extern "x86-interrupt" fn non_maskable_interrupt_handler(frame: InterruptStackFrame) {
    fatal_unknown_exception(frame, 2, None);
}

pub(super) extern "x86-interrupt" fn overflow_handler(frame: InterruptStackFrame) {
    fatal_unknown_exception(frame, 4, None);
}

pub(super) extern "x86-interrupt" fn bound_range_exceeded_handler(frame: InterruptStackFrame) {
    fatal_unknown_exception(frame, 5, None);
}

pub(super) extern "x86-interrupt" fn invalid_opcode_handler(frame: InterruptStackFrame) {
    handle_exception(interrupts::handle_exception(
        ExceptionDetail::InvalidOpcode,
        frame_view(&frame),
    ));
}

pub(super) extern "x86-interrupt" fn device_not_available_handler(frame: InterruptStackFrame) {
    fatal_unknown_exception(frame, 7, None);
}

pub(super) extern "x86-interrupt" fn double_fault_handler(
    frame: InterruptStackFrame,
    error_code: u64,
) -> ! {
    handle_exception(interrupts::handle_exception(
        ExceptionDetail::DoubleFault { error_code },
        frame_view(&frame),
    ));
}

pub(super) extern "x86-interrupt" fn invalid_tss_handler(
    frame: InterruptStackFrame,
    error_code: u64,
) {
    fatal_unknown_exception(frame, 10, Some(error_code));
}

pub(super) extern "x86-interrupt" fn segment_not_present_handler(
    frame: InterruptStackFrame,
    error_code: u64,
) {
    fatal_unknown_exception(frame, 11, Some(error_code));
}

pub(super) extern "x86-interrupt" fn stack_segment_fault_handler(
    frame: InterruptStackFrame,
    error_code: u64,
) {
    fatal_unknown_exception(frame, 12, Some(error_code));
}

pub(super) extern "x86-interrupt" fn general_protection_fault_handler(
    frame: InterruptStackFrame,
    error_code: u64,
) {
    handle_exception(interrupts::handle_exception(
        ExceptionDetail::GeneralProtection { error_code },
        frame_view(&frame),
    ));
}

pub(super) extern "x86-interrupt" fn page_fault_handler(
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

pub(super) extern "x86-interrupt" fn x87_floating_point_handler(frame: InterruptStackFrame) {
    fatal_unknown_exception(frame, 16, None);
}

pub(super) extern "x86-interrupt" fn alignment_check_handler(
    frame: InterruptStackFrame,
    error_code: u64,
) {
    fatal_unknown_exception(frame, 17, Some(error_code));
}

pub(super) extern "x86-interrupt" fn machine_check_handler(frame: InterruptStackFrame) -> ! {
    fatal_unknown_exception(frame, 18, None)
}

pub(super) extern "x86-interrupt" fn simd_floating_point_handler(frame: InterruptStackFrame) {
    fatal_unknown_exception(frame, 19, None);
}

pub(super) extern "x86-interrupt" fn virtualization_handler(frame: InterruptStackFrame) {
    fatal_unknown_exception(frame, 20, None);
}

pub(super) extern "x86-interrupt" fn control_protection_handler(
    frame: InterruptStackFrame,
    error_code: u64,
) {
    fatal_unknown_exception(frame, 21, Some(error_code));
}

pub(super) extern "x86-interrupt" fn hypervisor_injection_handler(frame: InterruptStackFrame) {
    fatal_unknown_exception(frame, 28, None);
}

pub(super) extern "x86-interrupt" fn vmm_communication_handler(
    frame: InterruptStackFrame,
    error_code: u64,
) {
    fatal_unknown_exception(frame, 29, Some(error_code));
}

pub(super) extern "x86-interrupt" fn security_exception_handler(
    frame: InterruptStackFrame,
    error_code: u64,
) {
    fatal_unknown_exception(frame, 30, Some(error_code));
}
