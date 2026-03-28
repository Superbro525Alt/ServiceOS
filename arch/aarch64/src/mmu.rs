use serviceos_kernel_core::{
    bootstrap::BootInfo,
    memory::{
        EarlyFrameAllocator, Frame, MappingError, MappingFlags, PageMapper, PhysicalAddress,
        VirtualAddress,
    },
};

#[cfg(target_arch = "aarch64")]
mod imp {
    use core::{cell::UnsafeCell, ptr};

    use spin::Once;

    use super::*;

    const PAGE_TABLE_ENTRIES: usize = 512;
    const PAGE_TABLE_PAGES: usize = 32;
    const PAGE_BYTES: u64 = 4096;
    const L2_BLOCK_BYTES: u64 = 2 * 1024 * 1024;
    const DESC_VALID: u64 = 1 << 0;
    const DESC_TABLE_OR_PAGE: u64 = 1 << 1;
    const ATTR_DEVICE_NGNRNE: u64 = 0;
    const ATTR_NORMAL_WBWA: u64 = 1;
    const AP_EL1_RW_EL0_NONE: u64 = 0b00 << 6;
    const AP_EL1_RW_EL0_RW: u64 = 0b01 << 6;
    const AP_EL1_RO_EL0_NONE: u64 = 0b10 << 6;
    const AP_EL1_RO_EL0_RO: u64 = 0b11 << 6;
    const SH_NON_SHAREABLE: u64 = 0b00 << 8;
    const SH_INNER_SHAREABLE: u64 = 0b11 << 8;
    const DESC_AF: u64 = 1 << 10;
    const DESC_PXN: u64 = 1 << 53;
    const DESC_UXN: u64 = 1 << 54;
    const TABLE_ADDRESS_MASK: u64 = 0x0000_FFFF_FFFF_F000;
    const L0_INDEX_SHIFT: u64 = 39;
    const L1_INDEX_SHIFT: u64 = 30;
    const L2_INDEX_SHIFT: u64 = 21;
    const L3_INDEX_SHIFT: u64 = 12;
    const INDEX_MASK: u64 = 0x1ff;

    #[derive(Clone, Copy)]
    #[repr(align(4096))]
    struct PageTable {
        entries: [u64; PAGE_TABLE_ENTRIES],
    }

    impl PageTable {
        const EMPTY: Self = Self {
            entries: [0; PAGE_TABLE_ENTRIES],
        };
    }

    struct TablePool(UnsafeCell<[PageTable; PAGE_TABLE_PAGES]>);

    unsafe impl Sync for TablePool {}

    static TABLE_POOL: TablePool = TablePool(UnsafeCell::new([PageTable::EMPTY; PAGE_TABLE_PAGES]));
    static KERNEL_ROOT_FRAME: Once<PhysicalAddress> = Once::new();

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct MmioRegion {
        pub base: PhysicalAddress,
        pub size: usize,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum MmuBringupError {
        TablePoolExhausted,
        AddressAlignment,
        FrameAllocationFailed,
        Unsupported,
    }

    pub struct ActivePageTable {
        root_frame: PhysicalAddress,
    }

    pub struct OwnedPageTable {
        root_frame: PhysicalAddress,
    }

    impl ActivePageTable {
        pub fn initialize(
            boot_info: &BootInfo<'_>,
            mmio_regions: &[MmioRegion],
        ) -> Result<Self, MmuBringupError> {
            let mut pool = EarlyTablePool::new();
            let root = pool.allocate_table()?;

            for region in boot_info.memory_regions {
                let start = region.start.align_down(L2_BLOCK_BYTES).as_u64();
                let end = region.end.align_up(L2_BLOCK_BYTES).as_u64();
                if start >= end {
                    continue;
                }
                map_identity_block_range(PhysicalAddress::new(root as *mut PageTable as u64), start, end, false)?;
            }

            for region in mmio_regions.iter().copied() {
                let start = region.base.align_down(L2_BLOCK_BYTES).as_u64();
                let end = PhysicalAddress::new(region.base.as_u64().saturating_add(region.size as u64))
                    .align_up(L2_BLOCK_BYTES)
                    .as_u64();
                map_identity_block_range(PhysicalAddress::new(root as *mut PageTable as u64), start, end, true)?;
            }

            let root_frame = PhysicalAddress::new(root as *mut PageTable as u64);
            configure_translation(root_frame);
            let _ = KERNEL_ROOT_FRAME.call_once(|| root_frame);
            Ok(Self { root_frame })
        }
    }

    impl OwnedPageTable {
        pub unsafe fn new_user_space(
            kernel_root: PhysicalAddress,
            allocator: &mut EarlyFrameAllocator,
        ) -> Result<Self, MappingError> {
            let Some(frame) = allocator.allocate_4kib() else {
                return Err(MappingError::FrameAllocationFailed);
            };
            let root_frame = frame.base;
            let root_table = table_ptr(root_frame);
            zero_table(root_table);
            let kernel_table = table_ptr(kernel_root);
            root_table.entries.copy_from_slice(&kernel_table.entries);
            Ok(Self { root_frame })
        }

        pub fn root_frame(&self) -> PhysicalAddress {
            self.root_frame
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
            map_page_into(self.root_frame, page_start, frame, flags, allocator)
        }

        fn translate(&self, address: VirtualAddress) -> Option<PhysicalAddress> {
            translate_address(self.root_frame, address)
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
            map_page_into(self.root_frame, page_start, frame, flags, allocator)
        }

        fn translate(&self, address: VirtualAddress) -> Option<PhysicalAddress> {
            translate_address(self.root_frame, address)
        }
    }

    pub fn current_page_table_root() -> PhysicalAddress {
        let root: u64;
        unsafe {
            core::arch::asm!(
                "mrs {value}, ttbr0_el1",
                value = out(reg) root,
                options(nomem, nostack, preserves_flags)
            );
        }
        PhysicalAddress::new(root & TABLE_ADDRESS_MASK)
    }

    pub unsafe fn load_page_table_root(root: PhysicalAddress) {
        unsafe {
            core::arch::asm!(
                "msr ttbr0_el1, {value}",
                "dsb ish",
                "isb",
                value = in(reg) root.as_u64(),
                options(nostack)
            );
        }
    }

    fn configure_translation(root_frame: PhysicalAddress) {
        const MAIR: u64 = 0x0000_0000_0000_FF00;
        const TCR: u64 = 16
            | (0b01 << 8)
            | (0b01 << 10)
            | (0b11 << 12)
            | (0b00 << 14)
            | (1 << 23)
            | (0b101 << 32);

        unsafe {
            core::arch::asm!(
                "msr mair_el1, {mair}",
                "msr tcr_el1, {tcr}",
                "msr ttbr0_el1, {root}",
                "dsb ish",
                "isb",
                "mrs x9, sctlr_el1",
                "orr x9, x9, #(1 << 0)",
                "orr x9, x9, #(1 << 2)",
                "orr x9, x9, #(1 << 12)",
                "msr sctlr_el1, x9",
                "isb",
                mair = in(reg) MAIR,
                tcr = in(reg) TCR,
                root = in(reg) root_frame.as_u64(),
                out("x9") _,
                options(nostack)
            );
        }
    }

    fn map_identity_block_range(
        root_frame: PhysicalAddress,
        start: u64,
        end: u64,
        device: bool,
    ) -> Result<(), MmuBringupError> {
        let mut address = start;
        while address < end {
            let root = table_ptr(root_frame);
            let l1 = ensure_child_table(root, ((address >> L0_INDEX_SHIFT) & INDEX_MASK) as usize)?;
            let l2 = ensure_child_table(l1, ((address >> L1_INDEX_SHIFT) & INDEX_MASK) as usize)?;
            let l2_index = ((address >> L2_INDEX_SHIFT) & INDEX_MASK) as usize;
            l2.entries[l2_index] = block_descriptor(address, device);
            address = address.saturating_add(L2_BLOCK_BYTES);
        }
        Ok(())
    }

    fn map_page_into(
        root_frame: PhysicalAddress,
        page_start: VirtualAddress,
        frame: Frame,
        flags: MappingFlags,
        allocator: &mut EarlyFrameAllocator,
    ) -> Result<(), MappingError> {
        if page_start.as_u64() % PAGE_BYTES != 0 || frame.base.as_u64() % PAGE_BYTES != 0 {
            return Err(MappingError::AddressAlignment);
        }

        let root = table_ptr(root_frame);
        let l1 = ensure_allocator_child_table(root, l0_index(page_start), allocator)?;
        let l2 = ensure_allocator_child_table(l1, l1_index(page_start), allocator)?;
        let l3 = ensure_allocator_child_table(l2, l2_index(page_start), allocator)?;
        let index = l3_index(page_start);
        if l3.entries[index] & DESC_VALID != 0 {
            return Err(MappingError::AlreadyMapped);
        }
        l3.entries[index] = page_descriptor(frame.base.as_u64(), flags);
        flush_tlb();
        Ok(())
    }

    fn translate_address(root_frame: PhysicalAddress, address: VirtualAddress) -> Option<PhysicalAddress> {
        let root = table_ptr(root_frame);
        let l0_entry = root.entries[l0_index(address)];
        if l0_entry & DESC_VALID == 0 {
            return None;
        }
        if l0_entry & DESC_TABLE_OR_PAGE == 0 {
            let base = l0_entry & TABLE_ADDRESS_MASK;
            return Some(PhysicalAddress::new(base + (address.as_u64() & ((1 << L0_INDEX_SHIFT) - 1))));
        }

        let l1 = table_ptr(PhysicalAddress::new(l0_entry & TABLE_ADDRESS_MASK));
        let l1_entry = l1.entries[l1_index(address)];
        if l1_entry & DESC_VALID == 0 {
            return None;
        }
        if l1_entry & DESC_TABLE_OR_PAGE == 0 {
            let base = l1_entry & !((1 << L1_INDEX_SHIFT) - 1);
            return Some(PhysicalAddress::new(base + (address.as_u64() & ((1 << L1_INDEX_SHIFT) - 1))));
        }

        let l2 = table_ptr(PhysicalAddress::new(l1_entry & TABLE_ADDRESS_MASK));
        let l2_entry = l2.entries[l2_index(address)];
        if l2_entry & DESC_VALID == 0 {
            return None;
        }
        if l2_entry & DESC_TABLE_OR_PAGE == 0 {
            let base = l2_entry & !((1 << L2_INDEX_SHIFT) - 1);
            return Some(PhysicalAddress::new(base + (address.as_u64() & ((1 << L2_INDEX_SHIFT) - 1))));
        }

        let l3 = table_ptr(PhysicalAddress::new(l2_entry & TABLE_ADDRESS_MASK));
        let l3_entry = l3.entries[l3_index(address)];
        if l3_entry & DESC_VALID == 0 {
            return None;
        }
        if l3_entry & DESC_TABLE_OR_PAGE == 0 {
            return None;
        }
        Some(PhysicalAddress::new(
            (l3_entry & TABLE_ADDRESS_MASK) + (address.as_u64() & (PAGE_BYTES - 1)),
        ))
    }

    fn ensure_child_table(
        parent: &'static mut PageTable,
        index: usize,
    ) -> Result<&'static mut PageTable, MmuBringupError> {
        if parent.entries[index] & DESC_VALID == 0 {
            let table = EarlyTablePool::global_allocate()?;
            let table_address = table as *mut PageTable as u64;
            parent.entries[index] = DESC_VALID | DESC_TABLE_OR_PAGE | table_address;
        }
        Ok(table_ptr(PhysicalAddress::new(parent.entries[index] & TABLE_ADDRESS_MASK)))
    }

    fn ensure_allocator_child_table(
        parent: &'static mut PageTable,
        index: usize,
        allocator: &mut EarlyFrameAllocator,
    ) -> Result<&'static mut PageTable, MappingError> {
        if parent.entries[index] & DESC_VALID == 0 {
            let Some(frame) = allocator.allocate_4kib() else {
                return Err(MappingError::FrameAllocationFailed);
            };
            let table = table_ptr(frame.base);
            zero_table(table);
            parent.entries[index] = DESC_VALID | DESC_TABLE_OR_PAGE | frame.base.as_u64();
        }
        Ok(table_ptr(PhysicalAddress::new(parent.entries[index] & TABLE_ADDRESS_MASK)))
    }

    fn block_descriptor(physical: u64, device: bool) -> u64 {
        let attr_index = if device {
            ATTR_DEVICE_NGNRNE
        } else {
            ATTR_NORMAL_WBWA
        };
        let shareability = if device {
            SH_NON_SHAREABLE
        } else {
            SH_INNER_SHAREABLE
        };
        (physical & !((1 << L2_INDEX_SHIFT) - 1))
            | DESC_VALID
            | ((attr_index as u64) << 2)
            | AP_EL1_RW_EL0_NONE
            | shareability
            | DESC_AF
            | if device { DESC_PXN | DESC_UXN } else { 0 }
    }

    fn page_descriptor(physical: u64, flags: MappingFlags) -> u64 {
        let (ap, uxn) = if flags.contains(MappingFlags::USER_ACCESSIBLE) {
            if flags.contains(MappingFlags::WRITABLE) {
                (AP_EL1_RW_EL0_RW, 0)
            } else {
                (AP_EL1_RO_EL0_RO, 0)
            }
        } else if flags.contains(MappingFlags::WRITABLE) {
            (AP_EL1_RW_EL0_NONE, DESC_UXN)
        } else {
            (AP_EL1_RO_EL0_NONE, DESC_UXN)
        };
        let mut descriptor = (physical & TABLE_ADDRESS_MASK)
            | DESC_VALID
            | DESC_TABLE_OR_PAGE
            | ((ATTR_NORMAL_WBWA as u64) << 2)
            | ap
            | SH_INNER_SHAREABLE
            | DESC_AF
            | uxn;
        if !flags.contains(MappingFlags::EXECUTABLE) {
            descriptor |= DESC_PXN | DESC_UXN;
        }
        descriptor
    }

    fn l0_index(address: VirtualAddress) -> usize {
        ((address.as_u64() >> L0_INDEX_SHIFT) & INDEX_MASK) as usize
    }

    fn l1_index(address: VirtualAddress) -> usize {
        ((address.as_u64() >> L1_INDEX_SHIFT) & INDEX_MASK) as usize
    }

    fn l2_index(address: VirtualAddress) -> usize {
        ((address.as_u64() >> L2_INDEX_SHIFT) & INDEX_MASK) as usize
    }

    fn l3_index(address: VirtualAddress) -> usize {
        ((address.as_u64() >> L3_INDEX_SHIFT) & INDEX_MASK) as usize
    }

    fn table_ptr(frame: PhysicalAddress) -> &'static mut PageTable {
        unsafe { &mut *(frame.as_u64() as *mut PageTable) }
    }

    fn zero_table(table: &mut PageTable) {
        unsafe {
            ptr::write_bytes(table as *mut PageTable, 0, 1);
        }
    }

    fn flush_tlb() {
        unsafe {
            core::arch::asm!(
                "dsb ishst",
                "tlbi vmalle1",
                "dsb ish",
                "isb",
                options(nostack)
            );
        }
    }

    struct EarlyTablePool {
        next: usize,
    }

    impl EarlyTablePool {
        fn new() -> Self {
            let tables = unsafe { &mut *TABLE_POOL.0.get() };
            for table in tables.iter_mut() {
                table.entries.fill(0);
            }
            Self { next: 0 }
        }

        fn allocate_table(&mut self) -> Result<&'static mut PageTable, MmuBringupError> {
            let tables = unsafe { &mut *TABLE_POOL.0.get() };
            let Some(table) = tables.get_mut(self.next) else {
                return Err(MmuBringupError::TablePoolExhausted);
            };
            table.entries.fill(0);
            self.next += 1;
            Ok(unsafe { &mut *(table as *mut PageTable) })
        }

        fn global_allocate() -> Result<&'static mut PageTable, MmuBringupError> {
            static NEXT_INDEX: Once<spin::Mutex<usize>> = Once::new();
            let next = NEXT_INDEX.call_once(|| spin::Mutex::new(1));
            let mut guard = next.lock();
            let tables = unsafe { &mut *TABLE_POOL.0.get() };
            let Some(table) = tables.get_mut(*guard) else {
                return Err(MmuBringupError::TablePoolExhausted);
            };
            table.entries.fill(0);
            *guard += 1;
            Ok(unsafe { &mut *(table as *mut PageTable) })
        }
    }
}

#[cfg(not(target_arch = "aarch64"))]
mod imp {
    use super::*;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct MmioRegion {
        pub base: PhysicalAddress,
        pub size: usize,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum MmuBringupError {
        Unsupported,
    }

    pub struct ActivePageTable;
    pub struct OwnedPageTable;

    impl ActivePageTable {
        pub fn initialize(
            _boot_info: &BootInfo<'_>,
            _mmio_regions: &[MmioRegion],
        ) -> Result<Self, MmuBringupError> {
            Err(MmuBringupError::Unsupported)
        }
    }

    impl OwnedPageTable {
        pub unsafe fn new_user_space(
            _kernel_root: PhysicalAddress,
            _allocator: &mut EarlyFrameAllocator,
        ) -> Result<Self, MappingError> {
            Err(MappingError::Unsupported)
        }

        pub fn root_frame(&self) -> PhysicalAddress {
            PhysicalAddress::new(0)
        }
    }

    impl PageMapper for ActivePageTable {
        fn active_root_frame(&self) -> PhysicalAddress {
            PhysicalAddress::new(0)
        }

        fn map_page(
            &mut self,
            _page_start: VirtualAddress,
            _frame: Frame,
            _flags: MappingFlags,
            _allocator: &mut EarlyFrameAllocator,
        ) -> Result<(), MappingError> {
            Err(MappingError::Unsupported)
        }

        fn translate(&self, _address: VirtualAddress) -> Option<PhysicalAddress> {
            None
        }
    }

    impl PageMapper for OwnedPageTable {
        fn active_root_frame(&self) -> PhysicalAddress {
            PhysicalAddress::new(0)
        }

        fn map_page(
            &mut self,
            _page_start: VirtualAddress,
            _frame: Frame,
            _flags: MappingFlags,
            _allocator: &mut EarlyFrameAllocator,
        ) -> Result<(), MappingError> {
            Err(MappingError::Unsupported)
        }

        fn translate(&self, _address: VirtualAddress) -> Option<PhysicalAddress> {
            None
        }
    }

    pub fn current_page_table_root() -> PhysicalAddress {
        PhysicalAddress::new(0)
    }

    pub unsafe fn load_page_table_root(_root: PhysicalAddress) {}
}

pub use imp::{
    ActivePageTable, MmioRegion, MmuBringupError, OwnedPageTable, current_page_table_root,
    load_page_table_root,
};
