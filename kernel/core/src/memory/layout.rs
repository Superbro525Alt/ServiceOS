use super::{MemoryUse, VirtualAddress, VirtualMemoryRange};

pub const USER_SPACE_START: VirtualAddress = VirtualAddress::new(0x0000_0000_0000_1000);
pub const USER_SPACE_END: VirtualAddress = VirtualAddress::new(0x0000_8000_0000_0000);
pub const KERNEL_PHYSICAL_WINDOW_START: VirtualAddress = VirtualAddress::new(0xffff_8000_0000_0000);
pub const KERNEL_PHYSICAL_WINDOW_END: VirtualAddress = VirtualAddress::new(0xffff_c000_0000_0000);
pub const KERNEL_HEAP_START: VirtualAddress = VirtualAddress::new(0xffff_c100_0000_0000);
pub const KERNEL_HEAP_END: VirtualAddress = VirtualAddress::new(0xffff_c100_0080_0000);
pub const KERNEL_OBJECTS_START: VirtualAddress = VirtualAddress::new(0xffff_c200_0000_0000);
pub const KERNEL_OBJECTS_END: VirtualAddress = VirtualAddress::new(0xffff_c201_0000_0000);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KernelVirtualLayout {
    pub user_range: VirtualMemoryRange,
    pub physical_window: VirtualMemoryRange,
    pub heap: VirtualMemoryRange,
    pub kernel_objects: VirtualMemoryRange,
}

impl KernelVirtualLayout {
    pub const fn bootstrap_default() -> Self {
        Self {
            user_range: VirtualMemoryRange {
                start: USER_SPACE_START,
                end: USER_SPACE_END,
                use_class: MemoryUse::UserPrivate,
            },
            physical_window: VirtualMemoryRange {
                start: KERNEL_PHYSICAL_WINDOW_START,
                end: KERNEL_PHYSICAL_WINDOW_END,
                use_class: MemoryUse::KernelData,
            },
            heap: VirtualMemoryRange {
                start: KERNEL_HEAP_START,
                end: KERNEL_HEAP_END,
                use_class: MemoryUse::KernelHeap,
            },
            kernel_objects: VirtualMemoryRange {
                start: KERNEL_OBJECTS_START,
                end: KERNEL_OBJECTS_END,
                use_class: MemoryUse::KernelObjects,
            },
        }
    }

    pub const fn kernel_reserved_start(&self) -> VirtualAddress {
        self.physical_window.start
    }

    pub const fn kernel_reserved_end(&self) -> VirtualAddress {
        self.kernel_objects.end
    }
}
