#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct PhysicalAddress(u64);

impl PhysicalAddress {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct VirtualAddress(u64);

impl VirtualAddress {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageSize {
    Size4KiB,
    Size2MiB,
    Size1GiB,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryUse {
    KernelText,
    KernelData,
    KernelStack,
    UserPrivate,
    Shared,
    DeviceMmio,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicalMemoryRange {
    pub start: PhysicalAddress,
    pub end: PhysicalAddress,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VirtualMemoryRange {
    pub start: VirtualAddress,
    pub end: VirtualAddress,
    pub use_class: MemoryUse,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Frame {
    pub base: PhysicalAddress,
    pub size: PageSize,
}

/// Interface for the future frame allocator and reservation tracker.
pub trait PhysicalMemoryManager {
    fn reserve(&mut self, range: PhysicalMemoryRange);
    fn allocate_frame(&mut self, size: PageSize) -> Option<Frame>;
}

/// Interface for architecture-independent virtual address space management.
pub trait AddressSpaceManager {
    fn map_kernel_region(&mut self, range: VirtualMemoryRange) -> Result<(), MappingError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MappingError {
    UnsupportedInPhase0,
    Overlap,
    AddressAlignment,
}
