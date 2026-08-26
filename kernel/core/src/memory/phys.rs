use super::{Frame, InitializationError, PAGE_SIZE_BYTES, PageSize};
use crate::bootstrap::{BootContext, BootMemoryRegionKind};

const MAX_ALLOCATABLE_REGIONS: usize = 128;
/// Bounded pool for frames handed back by address-space teardown. Regions are
/// coalesced on insert so contiguous frees collapse into one slot; once the
/// pool is full, further frees are counted in `dropped_free_frames` instead
/// of being silently lost (they are gone for the allocator's lifetime).
const MAX_RECLAIMED_REGIONS: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FrameRegionCursor {
    start_frame: u64,
    end_frame_exclusive: u64,
    next_frame: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameAllocatorStats {
    pub allocatable_regions: usize,
    pub allocated_frames: u64,
    pub remaining_frames: u64,
    pub reclaimed_boot_service_frames: u64,
    pub freed_frames_recorded: u64,
    pub dropped_free_frames: u64,
}

#[derive(Debug)]
pub struct EarlyFrameAllocator {
    regions: [FrameRegionCursor; MAX_ALLOCATABLE_REGIONS],
    region_count: usize,
    active_region: usize,
    allocated_frames: u64,
    reclaimed_boot_service_frames: u64,
    reclaimed: [FrameRegionCursor; MAX_RECLAIMED_REGIONS],
    reclaimed_count: usize,
    freed_frames_recorded: u64,
    dropped_free_frames: u64,
}

impl EarlyFrameAllocator {
    pub fn from_boot_context(boot_context: &BootContext<'_>) -> Result<Self, InitializationError> {
        let mut regions = [FrameRegionCursor {
            start_frame: 0,
            end_frame_exclusive: 0,
            next_frame: 0,
        }; MAX_ALLOCATABLE_REGIONS];
        let mut region_count = 0usize;

        for region in boot_context.memory_regions {
            if !matches!(region.kind, BootMemoryRegionKind::Usable) {
                continue;
            }

            let start = region.start.align_up(PAGE_SIZE_BYTES).as_u64() / PAGE_SIZE_BYTES;
            let end = region.end.align_down(PAGE_SIZE_BYTES).as_u64() / PAGE_SIZE_BYTES;
            if end <= start {
                continue;
            }

            push_region(&mut regions, &mut region_count, start, end)?;
        }

        Ok(Self {
            regions,
            region_count,
            active_region: 0,
            allocated_frames: 0,
            reclaimed_boot_service_frames: 0,
            reclaimed: [FrameRegionCursor {
                start_frame: 0,
                end_frame_exclusive: 0,
                next_frame: 0,
            }; MAX_RECLAIMED_REGIONS],
            reclaimed_count: 0,
            freed_frames_recorded: 0,
            dropped_free_frames: 0,
        })
    }

    pub fn reclaim_boot_services(
        &mut self,
        boot_context: &BootContext<'_>,
    ) -> Result<u64, InitializationError> {
        let mut reclaimed_frames = 0u64;

        for region in boot_context.memory_regions {
            if !matches!(region.kind, BootMemoryRegionKind::BootServicesReclaimable) {
                continue;
            }

            let start = region.start.align_up(PAGE_SIZE_BYTES).as_u64() / PAGE_SIZE_BYTES;
            let end = region.end.align_down(PAGE_SIZE_BYTES).as_u64() / PAGE_SIZE_BYTES;
            if end <= start {
                continue;
            }

            push_region(&mut self.regions, &mut self.region_count, start, end)?;
            reclaimed_frames = reclaimed_frames.saturating_add(end - start);
        }

        self.reclaimed_boot_service_frames = self
            .reclaimed_boot_service_frames
            .saturating_add(reclaimed_frames);
        Ok(reclaimed_frames)
    }

    pub fn allocate_4kib(&mut self) -> Option<Frame> {
        // Serve reclaimed frames first so freed address-space memory is
        // actually reused instead of being stranded in the free pool.
        for index in 0..self.reclaimed_count {
            let region = &mut self.reclaimed[index];
            if region.next_frame < region.end_frame_exclusive {
                let frame_number = region.next_frame;
                region.next_frame += 1;
                self.allocated_frames += 1;
                return Some(Frame {
                    base: super::PhysicalAddress::new(frame_number * PAGE_SIZE_BYTES),
                    size: PageSize::Size4KiB,
                });
            }
        }

        while self.active_region < self.region_count {
            let region = &mut self.regions[self.active_region];
            if region.next_frame < region.end_frame_exclusive {
                let frame_number = region.next_frame;
                region.next_frame += 1;
                self.allocated_frames += 1;

                return Some(Frame {
                    base: super::PhysicalAddress::new(frame_number * PAGE_SIZE_BYTES),
                    size: PageSize::Size4KiB,
                });
            }

            self.active_region += 1;
        }

        None
    }

    /// Hand a 4 KiB frame back for reallocation. Returns `true` when the
    /// frame was recorded. Frames freed from address-space teardown are
    /// served before fresh regions so exited tasks stop leaking memory.
    pub fn free_4kib(&mut self, base: super::PhysicalAddress) -> bool {
        if base.as_u64() % PAGE_SIZE_BYTES != 0 {
            return false;
        }
        let frame_number = base.as_u64() / PAGE_SIZE_BYTES;

        // Merge with a neighbouring reclaimed region if one exists.
        let mut insert_at = self.reclaimed_count;
        for index in 0..self.reclaimed_count {
            let region = self.reclaimed[index];
            if region.end_frame_exclusive == frame_number {
                self.reclaimed[index].end_frame_exclusive += 1;
                self.absorb_right(index);
                self.freed_frames_recorded += 1;
                return true;
            }
            if region.start_frame == frame_number + 1 && region.next_frame == region.start_frame {
                // Prepend only when the region has not handed out any of its
                // frames yet; otherwise a separate slot keeps serving order
                // correct.
                self.reclaimed[index].start_frame = frame_number;
                self.reclaimed[index].next_frame = frame_number;
                self.absorb_left(index);
                self.freed_frames_recorded += 1;
                return true;
            }
            if region.start_frame > frame_number && insert_at == self.reclaimed_count {
                insert_at = index;
            }
        }

        if self.reclaimed_count == MAX_RECLAIMED_REGIONS {
            // Pool exhausted: the frame cannot be tracked, so count it as
            // permanently dropped rather than losing it silently.
            self.dropped_free_frames += 1;
            return false;
        }

        for index in (insert_at..self.reclaimed_count).rev() {
            self.reclaimed[index + 1] = self.reclaimed[index];
        }
        self.reclaimed[insert_at] = FrameRegionCursor {
            start_frame: frame_number,
            end_frame_exclusive: frame_number + 1,
            next_frame: frame_number,
        };
        self.reclaimed_count += 1;
        self.freed_frames_recorded += 1;
        true
    }

    /// Fold the region at `index` into its right neighbour when contiguous.
    fn absorb_right(&mut self, index: usize) {
        if index + 1 < self.reclaimed_count
            && self.reclaimed[index].end_frame_exclusive == self.reclaimed[index + 1].start_frame
        {
            self.reclaimed[index].end_frame_exclusive =
                self.reclaimed[index + 1].end_frame_exclusive;
            self.remove_reclaimed(index + 1);
        }
    }

    /// Fold the region at `index` into its left neighbour when contiguous.
    fn absorb_left(&mut self, index: usize) {
        if index > 0
            && self.reclaimed[index - 1].end_frame_exclusive == self.reclaimed[index].start_frame
        {
            self.reclaimed[index - 1].end_frame_exclusive =
                self.reclaimed[index].end_frame_exclusive;
            self.remove_reclaimed(index);
        }
    }

    fn remove_reclaimed(&mut self, index: usize) {
        for cursor in index..self.reclaimed_count.saturating_sub(1) {
            self.reclaimed[cursor] = self.reclaimed[cursor + 1];
        }
        self.reclaimed_count -= 1;
    }

    pub fn remaining_frames(&self) -> u64 {
        self.regions[..self.region_count]
            .iter()
            .map(|region| region.end_frame_exclusive - region.next_frame)
            .sum()
    }

    /// Frames sitting in the reclaimed pool awaiting reallocation.
    pub fn reclaimable_frames(&self) -> u64 {
        self.reclaimed[..self.reclaimed_count]
            .iter()
            .map(|region| region.end_frame_exclusive - region.next_frame)
            .sum()
    }

    /// Total allocatable headroom: fresh regions plus the reclaimed pool.
    pub fn usable_headroom_frames(&self) -> u64 {
        self.remaining_frames() + self.reclaimable_frames()
    }

    pub fn stats(&self) -> FrameAllocatorStats {
        FrameAllocatorStats {
            allocatable_regions: self.region_count,
            allocated_frames: self.allocated_frames,
            remaining_frames: self.remaining_frames(),
            reclaimed_boot_service_frames: self.reclaimed_boot_service_frames,
            freed_frames_recorded: self.freed_frames_recorded,
            dropped_free_frames: self.dropped_free_frames,
        }
    }
}

fn push_region(
    regions: &mut [FrameRegionCursor; MAX_ALLOCATABLE_REGIONS],
    region_count: &mut usize,
    start_frame: u64,
    end_frame_exclusive: u64,
) -> Result<(), InitializationError> {
    if *region_count == regions.len() {
        return Err(InitializationError::TooManyUsableRegions);
    }

    regions[*region_count] = FrameRegionCursor {
        start_frame,
        end_frame_exclusive,
        next_frame: start_frame,
    };
    *region_count += 1;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bootstrap::{BootContext, BootMemoryRegion},
        memory::PhysicalAddress,
    };

    fn boot_context(regions: &[BootMemoryRegion]) -> BootContext<'_> {
        BootContext {
            memory_regions: regions,
            memory_map_available: true,
            memory_map_truncated: false,
            physical_memory_offset: None,
            rsdp_address: None,
            framebuffer: None,
            boot_store: None,
        }
    }

    #[test]
    fn reclaimed_pool_frames_count_toward_usable_headroom() {
        let mut allocator =
            EarlyFrameAllocator::from_boot_context(&boot_context(&[BootMemoryRegion {
                start: PhysicalAddress::new(0x1000),
                end: PhysicalAddress::new(0x4000),
                kind: BootMemoryRegionKind::Usable,
            }]))
            .expect("allocator");

        assert_eq!(allocator.usable_headroom_frames(), 3);
        let first = allocator.allocate_4kib().expect("frame").base;
        let second = allocator.allocate_4kib().expect("frame").base;
        let third = allocator.allocate_4kib().expect("frame").base;
        assert_eq!(allocator.remaining_frames(), 0);
        assert_eq!(allocator.reclaimable_frames(), 0);
        assert_eq!(allocator.usable_headroom_frames(), 0);

        assert!(allocator.free_4kib(first));
        assert!(allocator.free_4kib(second));
        assert!(allocator.free_4kib(third));
        // remaining_frames() keeps its fresh-region-only meaning; the
        // reclaimed pool is counted by reclaimable/usable-headroom.
        assert_eq!(allocator.remaining_frames(), 0);
        assert_eq!(allocator.reclaimable_frames(), 3);
        assert_eq!(allocator.usable_headroom_frames(), 3);
    }

    #[test]
    fn allocator_walks_usable_regions_in_order() {
        let regions = [
            BootMemoryRegion {
                start: PhysicalAddress::new(0x1003),
                end: PhysicalAddress::new(0x3000),
                kind: BootMemoryRegionKind::Usable,
            },
            BootMemoryRegion {
                start: PhysicalAddress::new(0x3000),
                end: PhysicalAddress::new(0x5000),
                kind: BootMemoryRegionKind::Reserved,
            },
            BootMemoryRegion {
                start: PhysicalAddress::new(0x8000),
                end: PhysicalAddress::new(0xA000),
                kind: BootMemoryRegionKind::Usable,
            },
        ];
        let mut allocator =
            EarlyFrameAllocator::from_boot_context(&boot_context(&regions)).expect("allocator");

        assert_eq!(
            allocator.allocate_4kib().expect("frame").base.as_u64(),
            0x2000
        );
        assert_eq!(
            allocator.allocate_4kib().expect("frame").base.as_u64(),
            0x8000
        );
        assert_eq!(
            allocator.allocate_4kib().expect("frame").base.as_u64(),
            0x9000
        );
        assert!(allocator.allocate_4kib().is_none());
        assert_eq!(
            allocator.stats(),
            FrameAllocatorStats {
                allocatable_regions: 2,
                allocated_frames: 3,
                remaining_frames: 0,
                reclaimed_boot_service_frames: 0,
                freed_frames_recorded: 0,
                dropped_free_frames: 0,
            }
        );
    }

    #[test]
    fn freed_frames_are_reused_before_fresh_regions() {
        let mut allocator =
            EarlyFrameAllocator::from_boot_context(&boot_context(&[BootMemoryRegion {
                start: PhysicalAddress::new(0x1000),
                end: PhysicalAddress::new(0x8000),
                kind: BootMemoryRegionKind::Usable,
            }]))
            .expect("allocator");

        let first = allocator.allocate_4kib().expect("frame").base;
        let second = allocator.allocate_4kib().expect("frame").base;
        let third = allocator.allocate_4kib().expect("frame").base;

        assert!(allocator.free_4kib(second));
        // Prepending `first` merges into the freed run so frames come back in
        // ascending order before any fresh region is touched.
        assert!(allocator.free_4kib(first));
        assert_eq!(allocator.stats().freed_frames_recorded, 2);

        assert_eq!(allocator.allocate_4kib().expect("frame").base, first);
        assert_eq!(allocator.allocate_4kib().expect("frame").base, second);
        // Pool drained: fresh frames come from the walk region.
        let fresh = allocator.allocate_4kib().expect("frame").base;
        assert_eq!(fresh, PhysicalAddress::new(0x4000));
        assert!(allocator.free_4kib(third));
        assert_eq!(
            allocator.allocate_4kib().expect("frame").base,
            third,
            "reclaimed pool must be served before fresh regions"
        );
    }

    #[test]
    fn misaligned_and_pool_overflow_frees_are_counted() {
        let mut allocator =
            EarlyFrameAllocator::from_boot_context(&boot_context(&[BootMemoryRegion {
                start: PhysicalAddress::new(0x1000),
                end: PhysicalAddress::new(0x2000),
                kind: BootMemoryRegionKind::Usable,
            }]))
            .expect("allocator");

        assert!(!allocator.free_4kib(PhysicalAddress::new(0x1801)));

        for index in 0..MAX_RECLAIMED_REGIONS + 8 {
            let base = PhysicalAddress::new(0x10_0000 + (index as u64) * PAGE_SIZE_BYTES * 2);
            let recorded = allocator.free_4kib(base);
            assert_eq!(recorded, index < MAX_RECLAIMED_REGIONS);
        }
        assert_eq!(allocator.stats().dropped_free_frames, 8);
    }

    #[test]
    fn allocator_rejects_excess_usable_regions() {
        let mut regions = [BootMemoryRegion {
            start: PhysicalAddress::new(0),
            end: PhysicalAddress::new(0),
            kind: BootMemoryRegionKind::Reserved,
        }; MAX_ALLOCATABLE_REGIONS + 1];

        for (index, region) in regions.iter_mut().enumerate() {
            let base = 0x1000 * ((index as u64) + 1);
            *region = BootMemoryRegion {
                start: PhysicalAddress::new(base),
                end: PhysicalAddress::new(base + PAGE_SIZE_BYTES),
                kind: BootMemoryRegionKind::Usable,
            };
        }

        assert!(matches!(
            EarlyFrameAllocator::from_boot_context(&boot_context(&regions)),
            Err(InitializationError::TooManyUsableRegions)
        ));
    }
}
