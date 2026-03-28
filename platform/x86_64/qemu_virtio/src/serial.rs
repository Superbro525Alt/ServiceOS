use core::fmt::{self, Write};
use x86_64::instructions::port::Port;

const COM1_BASE: u16 = 0x3F8;

pub fn init() {
    unsafe {
        write_port(COM1_BASE + 1, 0x00);
        write_port(COM1_BASE + 3, 0x80);
        write_port(COM1_BASE, 0x03);
        write_port(COM1_BASE + 1, 0x00);
        write_port(COM1_BASE + 3, 0x03);
        write_port(COM1_BASE + 2, 0xC7);
        write_port(COM1_BASE + 4, 0x0B);
    }
}

pub fn write_line(line: &str) {
    write_args(format_args!("{line}\n"));
}

pub fn write_args(args: fmt::Arguments<'_>) {
    let mut writer = SerialWriter;
    let _ = writer.write_fmt(args);
}

pub fn write_bytes(bytes: &[u8]) {
    for byte in bytes.iter().copied() {
        write_byte(byte);
    }
}

pub fn try_read_byte() -> Option<u8> {
    unsafe {
        if read_port(COM1_BASE + 5) & 0x01 == 0 {
            None
        } else {
            Some(read_port(COM1_BASE))
        }
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
        while read_port(COM1_BASE + 5) & 0x20 == 0 {}

        write_port(COM1_BASE, byte);
    }
}

unsafe fn write_port(port: u16, value: u8) {
    let mut serial_port = Port::<u8>::new(port);
    unsafe {
        serial_port.write(value);
    }
}

unsafe fn read_port(port: u16) -> u8 {
    let mut serial_port = Port::<u8>::new(port);
    unsafe { serial_port.read() }
}
