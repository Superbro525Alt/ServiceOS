//! Legacy-BIOS (SeaBIOS) x86_64 platform support.
//!
//! This crate intentionally contains only the pieces the freestanding
//! `qemu_isa` kernel image needs before the generic kernel takes over:
//! COM1 serial I/O helpers. All device backends (display, input, network,
//! block, audio) are absent on this platform; userspace runs serial-first.

#![no_std]

use core::fmt::{self, Write};

use x86_64::instructions::port::Port;

const COM1_BASE: u16 = 0x3F8;

pub fn init() {
    unsafe {
        let mut interrupt_enable = Port::<u8>::new(COM1_BASE + 1);
        interrupt_enable.write(0x00);
        let mut line_control = Port::<u8>::new(COM1_BASE + 3);
        line_control.write(0x80);
        let mut divisor_low = Port::<u8>::new(COM1_BASE);
        divisor_low.write(0x01);
        let mut divisor_high = Port::<u8>::new(COM1_BASE + 1);
        divisor_high.write(0x00);
        line_control.write(0x03);
        let mut fifo_control = Port::<u8>::new(COM1_BASE + 2);
        fifo_control.write(0xC7);
        let mut modem_control = Port::<u8>::new(COM1_BASE + 4);
        modem_control.write(0x0B);
    }
}

pub fn write_args(args: fmt::Arguments<'_>) {
    let mut writer = SerialWriter;
    let _ = writer.write_fmt(args);
}

pub fn write_bytes(bytes: &[u8]) {
    for byte in bytes {
        write_byte(*byte);
    }
}

pub fn try_read_byte() -> Option<u8> {
    unsafe {
        let mut line_status = Port::<u8>::new(COM1_BASE + 5);
        if line_status.read() & 0x01 == 0 {
            return None;
        }
        let mut data = Port::<u8>::new(COM1_BASE);
        Some(data.read())
    }
}

struct SerialWriter;

impl Write for SerialWriter {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for byte in text.bytes() {
            write_byte(byte);
        }

        Ok(())
    }
}

fn write_byte(byte: u8) {
    unsafe {
        let mut line_status = Port::<u8>::new(COM1_BASE + 5);
        while line_status.read() & 0x20 == 0 {}
        let mut data = Port::<u8>::new(COM1_BASE);
        data.write(byte);
    }
}

/// Namespace shim so the kernel image can `use serviceos_platform_qemu_isa::serial`.
pub mod serial {
    pub use super::{init, try_read_byte, write_args, write_bytes};
}
