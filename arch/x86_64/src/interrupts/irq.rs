use serviceos_kernel_core::interrupts::{self, InterruptVector};
use x86_64::structures::idt::InterruptStackFrame;

use super::{
    EXTERNAL_IRQ_HANDLERS, EXTERNAL_IRQ_LINES, PIC_PRIMARY_OFFSET, TIMER_TICK_HOOK, TIMER_VECTOR,
    pic::{acknowledge_pic, unmask_pic_irq_line},
};

pub(super) extern "x86-interrupt" fn timer_interrupt_handler(_frame: InterruptStackFrame) {
    let _ = interrupts::note_timer_interrupt(InterruptVector(TIMER_VECTOR as u16));
    if let Some(hook) = *TIMER_TICK_HOOK.lock() {
        hook();
    }
    acknowledge_pic(TIMER_VECTOR);
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
    acknowledge_pic(vector);
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
