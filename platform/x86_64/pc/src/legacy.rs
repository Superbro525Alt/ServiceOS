use x86_64::instructions::port::Port;

use serviceos_kernel_arch_x86_64::interrupts::{EXTERNAL_IRQ_VECTOR_BASE, ExternalInterruptOps};

const PIC_PRIMARY_OFFSET: u8 = EXTERNAL_IRQ_VECTOR_BASE;
const PIC_SECONDARY_OFFSET: u8 = EXTERNAL_IRQ_VECTOR_BASE + 8;

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

/// The [`ExternalInterruptOps`] implementation for the legacy PC setup shared
/// by both x86 platform images. Pass this to
/// [`serviceos_kernel_arch_x86_64::interrupts::initialize`].
pub fn external_ops() -> &'static ExternalInterruptOps {
    &EXTERNAL
}

static EXTERNAL: ExternalInterruptOps = ExternalInterruptOps {
    bring_up: initialize_pic,
    program_tick_source: initialize_pit,
    mask_line: mask_pic_irq_line,
    unmask_line: unmask_pic_irq_line,
    acknowledge_vector: acknowledge_pic,
    wait_tick_wraps: pit_wait_for_tick_wraps,
};

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

fn mask_pic_irq_line(irq_line: u8) {
    let mut pic1_data = Port::<u8>::new(PIC1_DATA_PORT);
    let mut pic2_data = Port::<u8>::new(PIC2_DATA_PORT);

    unsafe {
        if irq_line < 8 {
            let mask = pic1_data.read() | (1u8 << irq_line);
            pic1_data.write(mask);
            return;
        }

        let secondary_line = irq_line - 8;
        let mask = pic2_data.read() | (1u8 << secondary_line);
        pic2_data.write(mask);
    }
}

/// Latch and read the PIT channel 0 countdown without disturbing it.
fn latch_pit_channel0() -> u16 {
    let mut pit_command = Port::<u8>::new(PIT_COMMAND_PORT);
    let mut pit_channel0 = Port::<u8>::new(PIT_CHANNEL0_PORT);

    unsafe {
        pit_command.write(0x00);
        let low = pit_channel0.read();
        let high = pit_channel0.read();
        u16::from_le_bytes([low, high])
    }
}

/// Busy-wait until the running PIT has wrapped (reloaded) `wraps` times.
///
/// Each wrap marks one full PIT period, giving callers an interrupt-free
/// reference interval for calibrating other timers. Returns false on timeout
/// (PIT not counting).
fn pit_wait_for_tick_wraps(wraps: u32) -> bool {
    let mut previous = latch_pit_channel0();
    let mut seen_wraps = 0u32;
    let mut guard = 0u64;

    while seen_wraps < wraps {
        let current = latch_pit_channel0();
        // The counter decreases continuously and jumps back up on reload.
        if current > previous {
            seen_wraps += 1;
        }
        previous = current;
        guard += 1;
        if guard > 4_000_000 {
            return false;
        }
    }

    true
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

fn unmask_pic_irq_line(irq_line: u8) {
    let mut pic1_data = Port::<u8>::new(PIC1_DATA_PORT);
    let mut pic2_data = Port::<u8>::new(PIC2_DATA_PORT);

    unsafe {
        if irq_line < 8 {
            let mask = pic1_data.read() & !(1u8 << irq_line);
            pic1_data.write(mask);
            return;
        }

        let cascade_mask = pic1_data.read() & !(1u8 << 2);
        pic1_data.write(cascade_mask);
        let secondary_line = irq_line - 8;
        let mask = pic2_data.read() & !(1u8 << secondary_line);
        pic2_data.write(mask);
    }
}
