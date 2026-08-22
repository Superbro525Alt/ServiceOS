use crate::cpu;
use serviceos_kernel_core::memory::{
    EarlyFrameAllocator, Frame, MappingError, MappingFlags, PageMapper, PageSize, PhysicalAddress,
    VirtualAddress,
};
use x86_64::{
    PhysAddr, VirtAddr,
    registers::control::Cr3,
    structures::paging::{
        Mapper, OffsetPageTable, Page, PageSize as X86PageSize, PageTable, PageTableFlags,
        PhysFrame, Size4KiB, Translate,
        mapper::{FlagUpdateError, UnmapError},
    },
};

/// Physical memory offset for the kernel's direct map
/// Currently identity-mapped (offset 0) for boot compatibility.
const PHYSICAL_MEMORY_OFFSET: u64 = 0x0000_0000_0000_0000;

pub struct ActivePageTable {
    root_frame: PhysicalAddress,
    inner: OffsetPageTable<'static>,
}

pub struct OwnedPageTable {
    root_frame: PhysicalAddress,
    inner: OffsetPageTable<'static>,
}

impl ActivePageTable {
    pub unsafe fn new_identity_mapped() -> Self {
        let (root_frame, _) = Cr3::read();
        let root_address = PhysicalAddress::new(root_frame.start_address().as_u64());
        let level_4_table = page_table_ptr(root_address);

        Self {
            root_frame: root_address,
            inner: unsafe { OffsetPageTable::new(level_4_table, VirtAddr::new(0)) },
        }
    }

    /// Create a new active page table with the kernel's direct map
    ///
    /// # Safety
    /// This function must be called with interrupts disabled and the
    /// page table must be properly initialized.
    pub unsafe fn new_with_direct_map() -> Self {
        let (root_frame, _) = Cr3::read();
        let root_address = PhysicalAddress::new(root_frame.start_address().as_u64());
        let level_4_table = page_table_ptr(root_address);

        Self {
            root_frame: root_address,
            inner: unsafe {
                OffsetPageTable::new(level_4_table, VirtAddr::new(PHYSICAL_MEMORY_OFFSET))
            },
        }
    }
}

impl OwnedPageTable {
    pub unsafe fn new_user_space(
        kernel_root: PhysicalAddress,
        allocator: &mut EarlyFrameAllocator,
    ) -> Result<Self, MappingError> {
        let Some(root_frame) = allocator.allocate_4kib() else {
            return Err(MappingError::FrameAllocationFailed);
        };
        let root_frame = root_frame.base;
        let root_table = page_table_ptr(root_frame);
        root_table.zero();
        let kernel_table = page_table_ptr(kernel_root);
        let mut index = 0usize;
        while index < 512 {
            root_table[index] = kernel_table[index].clone();
            index += 1;
        }

        Ok(Self {
            root_frame,
            inner: unsafe {
                OffsetPageTable::new(root_table, VirtAddr::new(PHYSICAL_MEMORY_OFFSET))
            },
        })
    }

    /// Create a new kernel page table with the direct map
    ///
    /// # Safety
    /// This function must be called with interrupts disabled and the
    /// page table must be properly initialized.
    pub unsafe fn new_kernel_space(
        allocator: &mut EarlyFrameAllocator,
    ) -> Result<Self, MappingError> {
        let Some(root_frame) = allocator.allocate_4kib() else {
            return Err(MappingError::FrameAllocationFailed);
        };
        let root_frame = root_frame.base;
        let root_table = page_table_ptr(root_frame);
        root_table.zero();

        Ok(Self {
            root_frame,
            inner: unsafe {
                OffsetPageTable::new(root_table, VirtAddr::new(PHYSICAL_MEMORY_OFFSET))
            },
        })
    }

    pub fn root_frame(&self) -> PhysicalAddress {
        self.root_frame
    }

    pub unsafe fn from_root(root_frame: PhysicalAddress) -> Self {
        Self {
            root_frame,
            inner: unsafe {
                OffsetPageTable::new(
                    page_table_ptr(root_frame),
                    VirtAddr::new(PHYSICAL_MEMORY_OFFSET),
                )
            },
        }
    }

    /// Map a physical page into the kernel's direct map
    pub fn map_direct_map_page(
        &mut self,
        physical_address: PhysicalAddress,
        flags: MappingFlags,
        allocator: &mut EarlyFrameAllocator,
    ) -> Result<VirtualAddress, MappingError> {
        let virtual_address =
            VirtualAddress::new(physical_address.as_u64() + PHYSICAL_MEMORY_OFFSET);
        let frame = Frame {
            base: physical_address,
            size: PageSize::Size4KiB,
        };
        self.map_page(virtual_address, frame, flags, allocator)?;
        Ok(virtual_address)
    }

    /// Translate a virtual address in the direct map to a physical address
    pub fn translate_direct_map(&self, virtual_address: VirtualAddress) -> Option<PhysicalAddress> {
        let va = virtual_address.as_u64();
        if va >= PHYSICAL_MEMORY_OFFSET && va < 0xffff_c000_0000_0000 {
            Some(PhysicalAddress::new(va - PHYSICAL_MEMORY_OFFSET))
        } else {
            None
        }
    }
}

impl PageMapper for ActivePageTable {
    fn active_root_frame(&self) -> PhysicalAddress {
        self.root_frame
    }

    fn map_page(
        &mut self,
        page_start: VirtualAddress,
        frame: Frame,
        flags: MappingFlags,
        allocator: &mut EarlyFrameAllocator,
    ) -> Result<(), MappingError> {
        if page_start.as_u64() % Size4KiB::SIZE != 0 || frame.base.as_u64() % Size4KiB::SIZE != 0 {
            return Err(MappingError::AddressAlignment);
        }

        let page: Page<Size4KiB> = Page::from_start_address(VirtAddr::new(page_start.as_u64()))
            .map_err(|_| MappingError::AddressAlignment)?;
        let frame: PhysFrame<Size4KiB> =
            PhysFrame::from_start_address(PhysAddr::new(frame.base.as_u64()))
                .map_err(|_| MappingError::AddressAlignment)?;
        let mut adapter = FrameAllocatorAdapter { inner: allocator };
        let page_flags = to_page_table_flags(flags);
        let parent_flags = to_parent_table_flags(flags);

        cpu::with_write_protect_disabled(|| unsafe {
            self.inner
                .map_to_with_table_flags(page, frame, page_flags, parent_flags, &mut adapter)
                .map_err(|error| match error {
                    x86_64::structures::paging::mapper::MapToError::PageAlreadyMapped(_) => {
                        MappingError::AlreadyMapped
                    }
                    x86_64::structures::paging::mapper::MapToError::FrameAllocationFailed => {
                        MappingError::FrameAllocationFailed
                    }
                    _ => MappingError::Unsupported,
                })
                .map(|flush| flush.flush())
        })
    }

    fn translate(&self, address: VirtualAddress) -> Option<PhysicalAddress> {
        self.inner
            .translate_addr(VirtAddr::new(address.as_u64()))
            .map(|address| PhysicalAddress::new(address.as_u64()))
    }

    fn unmap_page(&mut self, page_start: VirtualAddress) -> Result<(), MappingError> {
        if page_start.as_u64() % Size4KiB::SIZE != 0 {
            return Err(MappingError::AddressAlignment);
        }
        let page: Page<Size4KiB> = Page::from_start_address(VirtAddr::new(page_start.as_u64()))
            .map_err(|_| MappingError::AddressAlignment)?;
        cpu::with_write_protect_disabled(|| {
            match self.inner.unmap(page) {
                Ok((_frame, flush)) => {
                    flush.flush();
                    Ok(())
                }
                Err(UnmapError::PageNotMapped) => Ok(()),
                Err(_) => Err(MappingError::Unsupported),
            }
        })
    }

    fn update_protection(
        &mut self,
        page_start: VirtualAddress,
        flags: MappingFlags,
    ) -> Result<(), MappingError> {
        if page_start.as_u64() % Size4KiB::SIZE != 0 {
            return Err(MappingError::AddressAlignment);
        }
        let page: Page<Size4KiB> = Page::from_start_address(VirtAddr::new(page_start.as_u64()))
            .map_err(|_| MappingError::AddressAlignment)?;
        let new_flags = to_page_table_flags(flags);
        cpu::with_write_protect_disabled(|| unsafe {
            match self.inner.update_flags(page, new_flags) {
                Ok(flush) => {
                    flush.flush();
                    Ok(())
                }
                Err(FlagUpdateError::PageNotMapped) => Err(MappingError::Unsupported),
                Err(_) => Err(MappingError::Unsupported),
            }
        })
    }
}

impl PageMapper for OwnedPageTable {
    fn active_root_frame(&self) -> PhysicalAddress {
        self.root_frame
    }

    fn map_page(
        &mut self,
        page_start: VirtualAddress,
        frame: Frame,
        flags: MappingFlags,
        allocator: &mut EarlyFrameAllocator,
    ) -> Result<(), MappingError> {
        if page_start.as_u64() % Size4KiB::SIZE != 0 || frame.base.as_u64() % Size4KiB::SIZE != 0 {
            return Err(MappingError::AddressAlignment);
        }

        let page: Page<Size4KiB> = Page::from_start_address(VirtAddr::new(page_start.as_u64()))
            .map_err(|_| MappingError::AddressAlignment)?;
        let frame: PhysFrame<Size4KiB> =
            PhysFrame::from_start_address(PhysAddr::new(frame.base.as_u64()))
                .map_err(|_| MappingError::AddressAlignment)?;
        let mut adapter = FrameAllocatorAdapter { inner: allocator };
        let page_flags = to_page_table_flags(flags);
        let parent_flags = to_parent_table_flags(flags);

        cpu::with_write_protect_disabled(|| unsafe {
            self.inner
                .map_to_with_table_flags(page, frame, page_flags, parent_flags, &mut adapter)
                .map_err(|error| match error {
                    x86_64::structures::paging::mapper::MapToError::PageAlreadyMapped(_) => {
                        MappingError::AlreadyMapped
                    }
                    x86_64::structures::paging::mapper::MapToError::FrameAllocationFailed => {
                        MappingError::FrameAllocationFailed
                    }
                    _ => MappingError::Unsupported,
                })
                .map(|flush| flush.flush())
        })
    }

    fn translate(&self, address: VirtualAddress) -> Option<PhysicalAddress> {
        self.inner
            .translate_addr(VirtAddr::new(address.as_u64()))
            .map(|address| PhysicalAddress::new(address.as_u64()))
    }

    fn unmap_page(&mut self, page_start: VirtualAddress) -> Result<(), MappingError> {
        if page_start.as_u64() % Size4KiB::SIZE != 0 {
            return Err(MappingError::AddressAlignment);
        }
        let page: Page<Size4KiB> = Page::from_start_address(VirtAddr::new(page_start.as_u64()))
            .map_err(|_| MappingError::AddressAlignment)?;
        cpu::with_write_protect_disabled(|| {
            match self.inner.unmap(page) {
                Ok((_frame, flush)) => {
                    flush.flush();
                    Ok(())
                }
                Err(UnmapError::PageNotMapped) => Ok(()),
                Err(_) => Err(MappingError::Unsupported),
            }
        })
    }

    fn update_protection(
        &mut self,
        page_start: VirtualAddress,
        flags: MappingFlags,
    ) -> Result<(), MappingError> {
        if page_start.as_u64() % Size4KiB::SIZE != 0 {
            return Err(MappingError::AddressAlignment);
        }
        let page: Page<Size4KiB> = Page::from_start_address(VirtAddr::new(page_start.as_u64()))
            .map_err(|_| MappingError::AddressAlignment)?;
        let new_flags = to_page_table_flags(flags);
        cpu::with_write_protect_disabled(|| unsafe {
            match self.inner.update_flags(page, new_flags) {
                Ok(flush) => {
                    flush.flush();
                    Ok(())
                }
                Err(FlagUpdateError::PageNotMapped) => Err(MappingError::Unsupported),
                Err(_) => Err(MappingError::Unsupported),
            }
        })
    }
}

struct FrameAllocatorAdapter<'a> {
    inner: &'a mut EarlyFrameAllocator,
}

unsafe impl x86_64::structures::paging::FrameAllocator<Size4KiB> for FrameAllocatorAdapter<'_> {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        self.inner.allocate_4kib().map(|frame| {
            unsafe {
                core::ptr::write_bytes(frame.base.as_u64() as *mut u8, 0, Size4KiB::SIZE as usize);
            }

            PhysFrame::from_start_address(PhysAddr::new(frame.base.as_u64()))
                .expect("allocated frames are always page aligned")
        })
    }
}

fn to_page_table_flags(flags: MappingFlags) -> PageTableFlags {
    let mut page_flags = PageTableFlags::PRESENT;
    if flags.contains(MappingFlags::WRITABLE) {
        page_flags |= PageTableFlags::WRITABLE;
    }
    if !flags.contains(MappingFlags::EXECUTABLE) {
        page_flags |= PageTableFlags::NO_EXECUTE;
    }
    if flags.contains(MappingFlags::USER_ACCESSIBLE) {
        page_flags |= PageTableFlags::USER_ACCESSIBLE;
    }
    if flags.contains(MappingFlags::GLOBAL) {
        page_flags |= PageTableFlags::GLOBAL;
    }
    page_flags
}

fn to_parent_table_flags(flags: MappingFlags) -> PageTableFlags {
    let mut parent_flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
    if flags.contains(MappingFlags::USER_ACCESSIBLE) {
        parent_flags |= PageTableFlags::USER_ACCESSIBLE;
    }

    parent_flags
}

fn page_table_ptr(root_frame: PhysicalAddress) -> &'static mut PageTable {
    unsafe { &mut *(root_frame.as_u64() as *mut PageTable) }
}
