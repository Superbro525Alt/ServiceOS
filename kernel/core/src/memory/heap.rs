use super::{
    EarlyFrameAllocator, InitializationError, MappingFlags, MemoryUse, PAGE_SIZE_BYTES, PageMapper,
    VirtualMemoryRange,
};
use core::alloc::{GlobalAlloc, Layout};
use core::ptr::null_mut;
use spin::Mutex;

#[global_allocator]
static KERNEL_ALLOCATOR: KernelAllocator = KernelAllocator::empty();

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
    inner: Mutex<BumpAllocator>,
}

impl KernelAllocator {
    const fn empty() -> Self {
        Self {
            inner: Mutex::new(BumpAllocator::empty()),
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
struct BumpAllocator {
    start: usize,
    end: usize,
    next: usize,
    active_allocations: usize,
    initialized: bool,
}

impl BumpAllocator {
    const fn empty() -> Self {
        Self {
            start: 0,
            end: 0,
            next: 0,
            active_allocations: 0,
            initialized: false,
        }
    }

    fn initialize(&mut self, start: usize, size: usize) -> Result<(), ()> {
        if self.initialized {
            return Err(());
        }

        self.start = start;
        self.end = start + size;
        self.next = start;
        self.active_allocations = 0;
        self.initialized = true;
        Ok(())
    }

    fn allocate(&mut self, layout: Layout) -> *mut u8 {
        if !self.initialized {
            return null_mut();
        }

        let start = align_up(self.next, layout.align());
        let Some(end) = start.checked_add(layout.size()) else {
            return null_mut();
        };
        if end > self.end {
            return null_mut();
        }

        self.next = end;
        self.active_allocations += 1;
        start as *mut u8
    }

    fn deallocate(&mut self, _ptr: *mut u8, _layout: Layout) {
        if self.active_allocations == 0 {
            return;
        }

        self.active_allocations -= 1;
        if self.active_allocations == 0 {
            self.next = self.start;
        }
    }
}

const fn align_up(value: usize, align: usize) -> usize {
    let mask = align - 1;
    (value + mask) & !mask
}
