use crate::cpu;
use serviceos_kernel_core::memory::{
    EarlyFrameAllocator, Frame, MappingError, MappingFlags, PageMapper, PhysicalAddress,
    VirtualAddress,
};
use x86_64::{
    PhysAddr, VirtAddr,
    registers::control::Cr3,
    structures::paging::{
        Mapper, OffsetPageTable, Page, PageSize, PageTable, PageTableFlags, PhysFrame, Size4KiB,
        Translate,
    },
};

pub struct ActivePageTable {
    root_frame: PhysicalAddress,
    inner: OffsetPageTable<'static>,
}

impl ActivePageTable {
    pub unsafe fn new_identity_mapped() -> Self {
        let (root_frame, _) = Cr3::read();
        let root_address = PhysicalAddress::new(root_frame.start_address().as_u64());
        let level_4_table = unsafe { &mut *(root_address.as_u64() as *mut PageTable) };

        Self {
            root_frame: root_address,
            inner: unsafe { OffsetPageTable::new(level_4_table, VirtAddr::new(0)) },
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

        cpu::with_write_protect_disabled(|| unsafe {
            self.inner
                .map_to(page, frame, to_page_table_flags(flags), &mut adapter)
                .map_err(|error| match error {
                    x86_64::structures::paging::mapper::MapToError::PageAlreadyMapped(_) => {
                        MappingError::AlreadyMapped
                    }
                    x86_64::structures::paging::mapper::MapToError::FrameAllocationFailed => {
                        MappingError::FrameAllocationFailed
                    }
                    _ => MappingError::UnsupportedInPhase1,
                })
                .map(|flush| flush.flush())
        })
    }

    fn translate(&self, address: VirtualAddress) -> Option<PhysicalAddress> {
        self.inner
            .translate_addr(VirtAddr::new(address.as_u64()))
            .map(|address| PhysicalAddress::new(address.as_u64()))
    }
}

struct FrameAllocatorAdapter<'a> {
    inner: &'a mut EarlyFrameAllocator,
}

unsafe impl x86_64::structures::paging::FrameAllocator<Size4KiB> for FrameAllocatorAdapter<'_> {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        self.inner.allocate_4kib().map(|frame| {
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
