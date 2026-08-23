#![no_std]
#![feature(abi_x86_interrupt)]

extern crate alloc;

pub mod acpi;
pub mod cpu;
pub mod hpet;
mod hpet_math;
pub mod interrupts;
pub mod kernel_context;
pub mod kthread;
pub mod lapic;
pub mod msr;
pub mod paging;
pub mod per_cpu;
mod serial;
pub mod smp;
pub mod user;
