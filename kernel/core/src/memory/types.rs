use core::ops::{BitOr, BitOrAssign};

pub const PAGE_SIZE_BYTES: u64 = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct PhysicalAddress(u64);

impl PhysicalAddress {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn as_u64(self) -> u64 {
        self.0
    }

    pub const fn align_up(self, align: u64) -> Self {
        let remainder = self.0 % align;
        if remainder == 0 {
            self
        } else {
            Self(self.0 + (align - remainder))
        }
    }

    pub const fn align_down(self, align: u64) -> Self {
        Self(self.0 - (self.0 % align))
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

    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }

    pub const fn offset(self, bytes: u64) -> Self {
        Self(self.0 + bytes)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageSize {
    Size4KiB,
    Size2MiB,
    Size1GiB,
}

impl PageSize {
    pub const fn bytes(self) -> u64 {
        match self {
            Self::Size4KiB => 4 * 1024,
            Self::Size2MiB => 2 * 1024 * 1024,
            Self::Size1GiB => 1024 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryUse {
    KernelText,
    KernelData,
    KernelStack,
    KernelHeap,
    KernelObjects,
    UserPrivate,
    Shared,
    DeviceMmio,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicalMemoryRange {
    pub start: PhysicalAddress,
    pub end: PhysicalAddress,
}

impl PhysicalMemoryRange {
    pub const fn size_bytes(self) -> u64 {
        self.end.as_u64() - self.start.as_u64()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VirtualMemoryRange {
    pub start: VirtualAddress,
    pub end: VirtualAddress,
    pub use_class: MemoryUse,
}

impl VirtualMemoryRange {
    pub const fn size_bytes(self) -> u64 {
        self.end.as_u64() - self.start.as_u64()
    }

    pub const fn page_count_4kib(self) -> usize {
        (self.size_bytes() / PAGE_SIZE_BYTES) as usize
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Frame {
    pub base: PhysicalAddress,
    pub size: PageSize,
}

impl Frame {
    pub const fn size_bytes(self) -> u64 {
        self.size.bytes()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MappingFlags(u8);

impl MappingFlags {
    pub const WRITABLE: Self = Self(1 << 0);
    pub const EXECUTABLE: Self = Self(1 << 1);
    pub const USER_ACCESSIBLE: Self = Self(1 << 2);
    pub const GLOBAL: Self = Self(1 << 3);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn kernel_data() -> Self {
        Self(Self::WRITABLE.0 | Self::GLOBAL.0)
    }
}

impl BitOr for MappingFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for MappingFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

pub trait PageMapper {
    fn active_root_frame(&self) -> PhysicalAddress;
    fn map_page(
        &mut self,
        page_start: VirtualAddress,
        frame: Frame,
        flags: MappingFlags,
        allocator: &mut super::EarlyFrameAllocator,
    ) -> Result<(), MappingError>;
    fn translate(&self, address: VirtualAddress) -> Option<PhysicalAddress>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MappingError {
    AddressAlignment,
    AlreadyMapped,
    FrameAllocationFailed,
    Unsupported,
}
