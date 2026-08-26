//! Memory-layout constants for the QEMU `virt` machine.
//!
//! The kernel runs bare-metal on the identity map with the MMU off (`satp`
//! stays in bare mode), so these physical addresses are also the executing
//! addresses. DRAM starts at 0x80000000 where QEMU places OpenSBI under
//! `-bios default`; the payload therefore links at 0x80200000, the entry the
//! firmware jumps to with a0=hart id and a1=DTB pointer in S-mode.

pub const DRAM_BASE: u64 = 0x8000_0000;
pub const OPENSBI_REGION_SIZE: u64 = 0x0020_0000;
pub const KERNEL_LOAD_BASE: u64 = DRAM_BASE + OPENSBI_REGION_SIZE;
pub const PAGE_SIZE: u64 = 4096;
pub const BOOT_STACK_SIZE: u64 = 64 * 1024;

pub const UART16550_BASE: u64 = 0x1000_0000;
pub const TEST_DEVICE_BASE: u64 = 0x1000_0000 + 0xF_0000;
