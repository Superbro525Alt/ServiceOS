use super::{
    EarlyFrameAllocator, InitializationError, MappingFlags, MemoryUse, PAGE_SIZE_BYTES, PageMapper,
    VirtualMemoryRange,
};
use core::alloc::{GlobalAlloc, Layout};
use core::ptr::null_mut;
use spin::Mutex;

#[cfg_attr(not(test), global_allocator)]
static KERNEL_ALLOCATOR: KernelAllocator = KernelAllocator::empty();
const MAX_FREE_REGIONS: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeapInfo {
    pub range: VirtualMemoryRange,
}

pub fn initialize_kernel_heap(
    mapper: &mut impl PageMapper,
    frame_allocator: &mut EarlyFrameAllocator,
    layout: &super::KernelVirtualLayout,
) -> Result<HeapInfo, InitializationError> {
    let heap_range = VirtualMemoryRange {
        start: layout.heap.start,
        end: layout.heap.end,
        use_class: MemoryUse::KernelHeap,
    };

    let mut next_page = heap_range.start;
    for _ in 0..heap_range.page_count_4kib() {
        let frame = frame_allocator
            .allocate_4kib()
            .ok_or(InitializationError::KernelHeapExhausted)?;
        mapper.map_page(
            next_page,
            frame,
            MappingFlags::kernel_data(),
            frame_allocator,
        )?;
        next_page = next_page.offset(PAGE_SIZE_BYTES);
    }

    unsafe {
        KERNEL_ALLOCATOR
            .initialize(
                heap_range.start.as_usize(),
                heap_range.size_bytes() as usize,
            )
            .map_err(|_| InitializationError::HeapAlreadyInitialized)?;
    }

    Ok(HeapInfo { range: heap_range })
}

struct KernelAllocator {
    inner: Mutex<FreeListAllocator>,
}

impl KernelAllocator {
    const fn empty() -> Self {
        Self {
            inner: Mutex::new(FreeListAllocator::empty()),
        }
    }

    unsafe fn initialize(&self, start: usize, size: usize) -> Result<(), ()> {
        let mut allocator = self.inner.lock();
        allocator.initialize(start, size)
    }
}

unsafe impl GlobalAlloc for KernelAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.inner.lock().allocate(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.inner.lock().deallocate(ptr, layout);
    }
}

#[derive(Debug)]
struct FreeListAllocator {
    regions: [FreeRegion; MAX_FREE_REGIONS],
    len: usize,
    initialized: bool,
    /// Bytes given up because the region table was full at free time.
    dropped_free_bytes: u64,
    /// Number of frees that could not be tracked (table full after
    /// coalescing). The affected bytes are permanently lost to the heap for
    /// its lifetime; counting them keeps the loss observable without
    /// panicking or growing the table.
    dropped_free_events: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FreeRegion {
    start: usize,
    size: usize,
}

impl FreeRegion {
    const fn empty() -> Self {
        Self { start: 0, size: 0 }
    }

    const fn end(&self) -> usize {
        self.start + self.size
    }
}

impl FreeListAllocator {
    const fn empty() -> Self {
        Self {
            regions: [FreeRegion::empty(); MAX_FREE_REGIONS],
            len: 0,
            initialized: false,
            dropped_free_bytes: 0,
            dropped_free_events: 0,
        }
    }

    fn initialize(&mut self, start: usize, size: usize) -> Result<(), ()> {
        if self.initialized {
            return Err(());
        }

        self.regions[0] = FreeRegion { start, size };
        self.len = 1;
        self.initialized = true;
        Ok(())
    }

    fn allocate(&mut self, layout: Layout) -> *mut u8 {
        if !self.initialized {
            return null_mut();
        }

        let size = layout.size().max(1);
        for index in 0..self.len {
            let region = self.regions[index];
            let start = align_up(region.start, layout.align());
            let Some(end) = start.checked_add(size) else {
                return null_mut();
            };
            if end > region.end() {
                continue;
            }

            let prefix_size = start - region.start;
            let suffix_size = region.end() - end;
            let suffix_start = end;

            match (prefix_size > 0, suffix_size > 0) {
                (false, false) => self.remove_region(index),
                (false, true) => {
                    self.regions[index] = FreeRegion {
                        start: suffix_start,
                        size: suffix_size,
                    };
                }
                (true, false) => {
                    self.regions[index].size = prefix_size;
                }
                (true, true) => {
                    if self.len == self.regions.len() {
                        return null_mut();
                    }
                    self.insert_region(
                        index + 1,
                        FreeRegion {
                            start: suffix_start,
                            size: suffix_size,
                        },
                    );
                    self.regions[index].size = prefix_size;
                }
            }

            return start as *mut u8;
        }

        null_mut()
    }

    fn deallocate(&mut self, ptr: *mut u8, layout: Layout) {
        if !self.initialized {
            return;
        }

        let start = ptr as usize;
        let Some(end) = start.checked_add(layout.size().max(1)) else {
            return;
        };
        let size = end - start;
        self.insert_and_coalesce(FreeRegion { start, size });
    }

    fn remove_region(&mut self, index: usize) {
        for cursor in index..self.len.saturating_sub(1) {
            self.regions[cursor] = self.regions[cursor + 1];
        }
        if self.len > 0 {
            self.len -= 1;
            self.regions[self.len] = FreeRegion::empty();
        }
    }

    fn insert_region(&mut self, index: usize, region: FreeRegion) {
        for cursor in (index..self.len).rev() {
            self.regions[cursor + 1] = self.regions[cursor];
        }
        self.regions[index] = region;
        self.len += 1;
    }

    fn insert_and_coalesce(&mut self, mut region: FreeRegion) {
        if region.size == 0 {
            return;
        }

        let mut index = 0usize;
        while index < self.len && self.regions[index].start < region.start {
            index += 1;
        }

        if index > 0 {
            let left = self.regions[index - 1];
            if left.end() >= region.start {
                let merged_start = left.start.min(region.start);
                let merged_end = left.end().max(region.end());
                region.start = merged_start;
                region.size = merged_end - merged_start;
                self.remove_region(index - 1);
                index -= 1;
            }
        }

        while index < self.len {
            let next = self.regions[index];
            if region.end() < next.start {
                break;
            }
            let merged_start = region.start.min(next.start);
            let merged_end = region.end().max(next.end());
            region.start = merged_start;
            region.size = merged_end - merged_start;
            self.remove_region(index);
        }

        if self.len == self.regions.len() {
            // Region table exhausted even after coalescing with every
            // neighbour. Never lose memory silently: account the bytes so the
            // leak is observable, keep the allocator running, and let
            // drop_stats() surface it.
            self.dropped_free_bytes = self.dropped_free_bytes.saturating_add(region.size as u64);
            self.dropped_free_events = self.dropped_free_events.saturating_add(1);
            return;
        }
        self.insert_region(index, region);
    }

    /// (events, bytes) of frees that the fixed region table could not track.
    /// Non-zero values mean the heap permanently lost those bytes because the
    /// 4096-slot table stayed full after coalescing; the alternative would be
    /// an unbounded table or a panic, both worse for a kernel heap.
    #[cfg(test)]
    pub fn drop_stats(&self) -> (u64, u64) {
        (self.dropped_free_events, self.dropped_free_bytes)
    }
}

const fn align_up(value: usize, align: usize) -> usize {
    let mask = align - 1;
    (value + mask) & !mask
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(size: usize, align: usize) -> Layout {
        Layout::from_size_align(size, align).unwrap()
    }

    #[test]
    fn allocator_reuses_freed_region_without_full_reset() {
        let mut allocator = FreeListAllocator::empty();
        allocator.initialize(0x1000, 0x4000).unwrap();

        let first = allocator.allocate(layout(64, 8));
        let second = allocator.allocate(layout(64, 8));
        assert!(!first.is_null());
        assert!(!second.is_null());

        allocator.deallocate(first, layout(64, 8));

        let reused = allocator.allocate(layout(32, 8));
        assert_eq!(reused, first);
    }

    #[test]
    fn allocator_coalesces_adjacent_regions() {
        let mut allocator = FreeListAllocator::empty();
        allocator.initialize(0x2000, 0x4000).unwrap();

        let a = allocator.allocate(layout(128, 8));
        let b = allocator.allocate(layout(128, 8));
        let c = allocator.allocate(layout(128, 8));

        allocator.deallocate(b, layout(128, 8));
        allocator.deallocate(a, layout(128, 8));
        allocator.deallocate(c, layout(128, 8));

        let whole = allocator.allocate(layout(384, 8));
        assert_eq!(whole, a);
    }

    #[test]
    fn allocator_respects_alignment_with_split_regions() {
        let mut allocator = FreeListAllocator::empty();
        allocator.initialize(0x3003, 0x4000).unwrap();

        let aligned = allocator.allocate(layout(160, 64));
        assert!(!aligned.is_null());
        assert_eq!((aligned as usize) % 64, 0);
    }
    #[test]
    fn full_region_table_counts_lost_bytes_instead_of_dropping_silently() {
        let mut allocator = FreeListAllocator::empty();
        allocator.initialize(0x1000_0000, 0x4000_0000).unwrap();

        // Saturate the region table with disjoint far-apart regions; the
        // first one is large enough to keep serving allocations.
        for index in 0..MAX_FREE_REGIONS {
            allocator.regions[index] = FreeRegion {
                start: 0x1000 + index * 0x10_0000,
                size: if index == 0 { 0x1000 } else { 16 },
            };
        }
        allocator.len = MAX_FREE_REGIONS;

        // One more non-adjacent free cannot be tracked: it must be counted,
        // not silently discarded.
        assert_eq!(allocator.drop_stats(), (0, 0));
        allocator.deallocate((0x8000_0000 as usize) as *mut u8, layout(16, 16));
        assert_eq!(allocator.drop_stats(), (1, 16));
        assert_eq!(allocator.len, MAX_FREE_REGIONS);

        // The heap keeps working afterwards.
        let retry = allocator.allocate(layout(32, 8));
        assert!(!retry.is_null());
    }

    #[test]
    fn allocator_survives_fragmentation_pressure() {
        let mut allocator = FreeListAllocator::empty();
        allocator.initialize(0x10_0000, 0x40_000).unwrap();

        let mut pointers = [null_mut(); 512];
        for (index, slot) in pointers.iter_mut().enumerate() {
            let align = if index % 2 == 0 { 8 } else { 16 };
            *slot = allocator.allocate(layout(64 + (index % 5) * 32, align));
            assert!(!slot.is_null());
        }

        for index in (0..pointers.len()).step_by(2) {
            allocator.deallocate(pointers[index], layout(64 + (index % 5) * 32, 8));
        }

        for index in (1..pointers.len()).step_by(2) {
            allocator.deallocate(pointers[index], layout(64 + (index % 5) * 32, 16));
        }

        let large = allocator.allocate(layout(0x8000, 16));
        assert!(!large.is_null());
    }
}
