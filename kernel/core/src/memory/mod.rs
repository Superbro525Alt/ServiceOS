mod address_space;
mod heap;
mod layout;
mod manager;
mod phys;
mod types;

pub use address_space::{AddressSpaceRoot, FutureUserAddressSpaceLayout, KernelAddressSpace};
pub use heap::HeapInfo;
pub use layout::KernelVirtualLayout;
pub use manager::{InitializationError, MemoryManager, MemoryStats, initialize, manager};
pub use phys::{EarlyFrameAllocator, FrameAllocatorStats};
pub use types::{
    Frame, MappingError, MappingFlags, MemoryUse, PAGE_SIZE_BYTES, PageMapper, PageSize,
    PhysicalAddress, PhysicalMemoryRange, VirtualAddress, VirtualMemoryRange,
};
