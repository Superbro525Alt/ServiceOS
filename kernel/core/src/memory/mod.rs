mod address_space;
mod heap;
mod layout;
mod manager;
pub mod oom;
mod phys;
pub mod pressure;
mod types;

pub use heap::{HeapInfo, HeapUsage, kernel_heap_usage};
pub use oom::{OOM_EXIT_CODE, OomError, VictimCandidate};
pub use pressure::{PressureLevel, PressureReading, PressureTransition};

pub use address_space::{AddressSpaceRoot, FutureUserAddressSpaceLayout, KernelAddressSpace};
pub use layout::{KernelVirtualLayout, USER_SPACE_END};
pub use manager::{InitializationError, MemoryManager, MemoryStats, initialize, manager};
pub use phys::{EarlyFrameAllocator, FrameAllocatorStats};
pub use types::{
    Frame, MappingError, MappingFlags, MemoryUse, PAGE_SIZE_BYTES, PageMapper, PageSize,
    PhysicalAddress, PhysicalMemoryRange, VirtualAddress, VirtualMemoryRange,
};
