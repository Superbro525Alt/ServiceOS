use super::{Frame, InitializationError, PAGE_SIZE_BYTES, PageSize};
use crate::bootstrap::{BootContext, BootMemoryRegionKind};

const MAX_ALLOCATABLE_REGIONS: usize = 128;

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
}

#[derive(Debug)]
pub struct EarlyFrameAllocator {
    regions: [FrameRegionCursor; MAX_ALLOCATABLE_REGIONS],
    region_count: usize,
    active_region: usize,
    allocated_frames: u64,
    reclaimed_boot_service_frames: u64,
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
            allocatable_regions: self.region_count,
            allocated_frames: self.allocated_frames,
            remaining_frames: self.remaining_frames(),
            reclaimed_boot_service_frames: self.reclaimed_boot_service_frames,
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
            }
        );
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
