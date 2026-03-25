use super::{Frame, InitializationError, PAGE_SIZE_BYTES, PageSize};
use crate::bootstrap::{BootContext, BootMemoryRegionKind};

const MAX_USABLE_REGIONS: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FrameRegionCursor {
    start_frame: u64,
    end_frame_exclusive: u64,
    next_frame: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameAllocatorStats {
    pub usable_regions: usize,
    pub allocated_frames: u64,
    pub remaining_frames: u64,
}

#[derive(Debug)]
pub struct EarlyFrameAllocator {
    regions: [FrameRegionCursor; MAX_USABLE_REGIONS],
    region_count: usize,
    active_region: usize,
    allocated_frames: u64,
}

impl EarlyFrameAllocator {
    pub fn from_boot_context(boot_context: &BootContext<'_>) -> Result<Self, InitializationError> {
        let mut regions = [FrameRegionCursor {
            start_frame: 0,
            end_frame_exclusive: 0,
            next_frame: 0,
        }; MAX_USABLE_REGIONS];
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

            if region_count == regions.len() {
                return Err(InitializationError::TooManyUsableRegions);
            }

            regions[region_count] = FrameRegionCursor {
                start_frame: start,
                end_frame_exclusive: end,
                next_frame: start,
            };
            region_count += 1;
        }

        Ok(Self {
            regions,
            region_count,
            active_region: 0,
            allocated_frames: 0,
        })
    }

    pub fn allocate_4kib(&mut self) -> Option<Frame> {
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

    pub fn remaining_frames(&self) -> u64 {
        self.regions[..self.region_count]
            .iter()
            .map(|region| region.end_frame_exclusive - region.next_frame)
            .sum()
    }

    pub fn stats(&self) -> FrameAllocatorStats {
        FrameAllocatorStats {
            usable_regions: self.region_count,
            allocated_frames: self.allocated_frames,
            remaining_frames: self.remaining_frames(),
        }
    }
}
