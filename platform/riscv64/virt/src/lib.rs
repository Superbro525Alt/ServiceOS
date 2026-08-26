//! QEMU `virt` machine support for the RISC-V skeleton target.
//!
//! Scope is intentionally minimal: memory-map constants and the sifive_test
//! finisher used to exit QEMU with a status code. No DTB parsing, device
//! drivers, or userspace plumbing yet — the SBI console owns early serial.
#![no_std]

pub mod machine;
