use serviceos_kernel_core::interrupts::{self, InterruptVector};
use serviceos_kernel_core::task;
use x86_64::structures::idt::InterruptStackFrame;

use super::{
    EXTERNAL_IRQ_HANDLERS, EXTERNAL_IRQ_LINES, PIC_PRIMARY_OFFSET, TIMER_VECTOR,
    pic::{acknowledge_pic, unmask_pic_irq_line},
};
use crate::user::SavedUserContext;

/// Raw timer IRQ frame. The asm stub pushes the 15 general-purpose registers
/// in `SavedUserContext` field order, then the CPU appends the 5-qword
/// interrupt frame (no error code for the timer vector), so a
/// `SavedUserContext` overlays the entire 160-byte image one-to-one: its
/// trailing instruction_pointer/code_segment/cpu_flags/user_stack_pointer/
/// user_stack_segment fields land exactly on the IRET tail.
#[repr(C)]
pub(super) struct TimerIrqFrame {
    pub context: SavedUserContext,
}

/// Rust body of the timer IRQ stub.
///
/// Returns 0 to resume the interrupted context via IRET, or 1 to abandon the
/// frame and return into `run_thread`'s kernel continuation because the
/// interrupted user thread is being preempted.
#[unsafe(no_mangle)]
extern "C" fn serviceos_x86_64_handle_timer_irq(frame: *mut TimerIrqFrame) -> u64 {
    // SAFETY: the asm stub passes the stack address of a fully populated
    // TimerIrqFrame and the frame outlives this call.
    let frame = unsafe { &mut *frame };

    let _ = interrupts::note_timer_interrupt(InterruptVector(TIMER_VECTOR as u16));

    // The system timer is the PIT routed through the 8259 PIC, so the PIC
    // must always be acknowledged or it will not deliver further timer
    // interrupts. The LAPIC is enabled as an interrupt controller in
    // virtual-wire mode and additionally requires an EOI for deliveries
    // that pass through it. This happens on both the resume and the
    // preemption path.
    unsafe {
        acknowledge_pic(TIMER_VECTOR);
        if crate::lapic::timer().is_initialized() {
            crate::lapic::send_eoi();
        }
    }

    if frame.context.code_segment & 0b11 != 0b11 {
        return 0;
    }

    let Some(tasks) = task::system() else {
        return 0;
    };
    let scheduler = tasks.scheduler();
    if !scheduler.preemption_pending() {
        return 0;
    }

    // The context fields already mirror the on-stack IRET tail; snapshot the
    // full user context before the frame is abandoned so the thread resumes
    // from this exact state when it is scheduled again via run_thread().
    let context = frame.context;

    if let Some(thread_id) = scheduler.current_thread() {
        crate::user::save_thread_context(thread_id, &context);
    }
    // Requeue the current thread and select its successor now so the executor
    // loop picks up the next thread as soon as run_thread() returns.
    // preempt_current_if_needed() checks and clears preemption_pending under
    // the scheduler lock; consuming the flag separately first would make it
    // early-return and leave the preempted thread in place forever.
    let _ = scheduler.preempt_current_if_needed();
    1
}

pub(super) extern "x86-interrupt" fn lapic_spurious_interrupt_handler(
    _frame: InterruptStackFrame,
) {
    // Spurious LAPIC interrupts must not receive an EOI (SDM 10.9).
    let _ = interrupts::note_external_interrupt(InterruptVector(
        crate::lapic::LAPIC_SPURIOUS_VECTOR as u16,
    ));
}

pub(super) fn register_external_irq_handler(irq_line: u8, handler: fn(u8)) -> bool {
    if irq_line as usize >= EXTERNAL_IRQ_LINES || irq_line == 0 {
        return false;
    }

    let mut handlers = EXTERNAL_IRQ_HANDLERS.lock();
    let line_handlers = &mut handlers[irq_line as usize];
    for existing in line_handlers.iter().flatten().copied() {
        if core::ptr::fn_addr_eq(existing, handler) {
            drop(handlers);
            unmask_pic_irq_line(irq_line);
            return true;
        }
    }
    if let Some(slot) = line_handlers.iter_mut().find(|slot| slot.is_none()) {
        *slot = Some(handler);
        drop(handlers);
        unmask_pic_irq_line(irq_line);
        return true;
    }
    false
}

fn dispatch_external_irq(irq_line: u8) {
    let vector = PIC_PRIMARY_OFFSET + irq_line;
    interrupts::note_external_interrupt(InterruptVector(vector as u16));
    let handlers = EXTERNAL_IRQ_HANDLERS.lock();
    for handler in handlers[irq_line as usize].iter().flatten().copied() {
        handler(irq_line);
    }
    // The LAPIC is enabled in virtual-wire mode, so external PIC deliveries
    // may pass through it; acknowledge both controllers like the timer path.
    unsafe {
        acknowledge_pic(vector);
        if crate::lapic::timer().is_initialized() {
            crate::lapic::send_eoi();
        }
    }
}

macro_rules! external_irq_handler {
    ($name:ident, $line:expr) => {
        pub(super) extern "x86-interrupt" fn $name(_frame: InterruptStackFrame) {
            dispatch_external_irq($line);
        }
    };
}

external_irq_handler!(external_irq1_handler, 1);
external_irq_handler!(external_irq2_handler, 2);
external_irq_handler!(external_irq3_handler, 3);
external_irq_handler!(external_irq4_handler, 4);
external_irq_handler!(external_irq5_handler, 5);
external_irq_handler!(external_irq6_handler, 6);
external_irq_handler!(external_irq7_handler, 7);
external_irq_handler!(external_irq8_handler, 8);
external_irq_handler!(external_irq9_handler, 9);
external_irq_handler!(external_irq10_handler, 10);
external_irq_handler!(external_irq11_handler, 11);
external_irq_handler!(external_irq12_handler, 12);
external_irq_handler!(external_irq13_handler, 13);
external_irq_handler!(external_irq14_handler, 14);
external_irq_handler!(external_irq15_handler, 15);
