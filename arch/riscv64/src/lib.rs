//! RISC-V (RV64) architecture support for ServiceOS.
//!
//! Skeleton scope (honest): SBI-console output, trap-vector setup with an
//! all-traps hang handler, timer access through the SBI TIME extension, and
//! memory-layout constants for the QEMU `virt` machine.
//!
//! There is no MMU support yet: the kernel runs bare-metal on the identity
//! map (`satp` untouched, mode 0/bare), exactly as the firmware hands off.
//! Traps, userspace, and paging are open roadmap work.
#![no_std]

pub mod console;
pub mod cpu;
pub mod layout;
pub mod sbi;
pub mod timer;
pub mod traps;
