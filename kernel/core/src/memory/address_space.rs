use super::{KernelVirtualLayout, PhysicalAddress};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AddressSpaceRoot {
    pub level_4_frame: PhysicalAddress,
}

impl AddressSpaceRoot {
    pub const fn new(level_4_frame: PhysicalAddress) -> Self {
        Self { level_4_frame }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FutureUserAddressSpaceLayout {
    pub user_space_start: super::VirtualAddress,
    pub user_space_end: super::VirtualAddress,
    pub kernel_shared_start: super::VirtualAddress,
    pub kernel_shared_end: super::VirtualAddress,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KernelAddressSpace {
    pub root: AddressSpaceRoot,
    pub layout: KernelVirtualLayout,
}

impl KernelAddressSpace {
    pub const fn new(root: AddressSpaceRoot, layout: KernelVirtualLayout) -> Self {
        Self { root, layout }
    }

    pub const fn future_user_layout(&self) -> FutureUserAddressSpaceLayout {
        FutureUserAddressSpaceLayout {
            user_space_start: self.layout.user_range.start,
            user_space_end: self.layout.user_range.end,
            kernel_shared_start: self.layout.kernel_reserved_start(),
            kernel_shared_end: self.layout.kernel_reserved_end(),
        }
    }
}
