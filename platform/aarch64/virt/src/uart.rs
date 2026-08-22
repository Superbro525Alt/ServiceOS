use core::sync::atomic::{AtomicU64, Ordering};

use serviceos_kernel_arch_aarch64::cpu;
use serviceos_kernel_core::memory::PhysicalAddress;

const PL011_DR: usize = 0x00;
const PL011_FR: usize = 0x18;
const PL011_FR_RXFE: u32 = 1 << 4;
const PL011_FR_TXFF: u32 = 1 << 5;

static UART_BASE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UartStatus {
    pub implemented: bool,
    pub initialized: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct UartDescriptor<'a> {
    pub path: &'a str,
    pub base: PhysicalAddress,
    pub span: usize,
    pub compatible: Option<&'a str>,
}

pub fn status() -> UartStatus {
    UartStatus {
        implemented: true,
        initialized: UART_BASE.load(Ordering::Relaxed) != 0,
    }
}

pub fn initialize(descriptor: UartDescriptor<'_>) {
    UART_BASE.store(descriptor.base.as_u64(), Ordering::Relaxed);
    cpu::data_synchronization_barrier();
    cpu::instruction_synchronization_barrier();
}

pub fn write_bytes(bytes: &[u8]) {
    for &byte in bytes {
        write_byte(byte);
    }
}

pub fn write_byte(byte: u8) {
    let base = UART_BASE.load(Ordering::Relaxed);
    if base == 0 {
        return;
    }

    while read_reg(base, PL011_FR) & PL011_FR_TXFF != 0 {}
    write_reg(base, PL011_DR, byte as u32);
}

pub fn try_read_byte() -> Option<u8> {
    let base = UART_BASE.load(Ordering::Relaxed);
    if base == 0 {
        return None;
    }
    if read_reg(base, PL011_FR) & PL011_FR_RXFE != 0 {
        return None;
    }
    Some((read_reg(base, PL011_DR) & 0xff) as u8)
}

fn read_reg(base: u64, offset: usize) -> u32 {
    unsafe { ((base + offset as u64) as *const u32).read_volatile() }
}

fn write_reg(base: u64, offset: usize, value: u32) {
    unsafe {
        ((base + offset as u64) as *mut u32).write_volatile(value);
    }
}
