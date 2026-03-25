# Memory Foundation

## Implemented in Phase 1

- UEFI memory-map normalization into architecture-neutral boot regions
- early frame allocation from `CONVENTIONAL` memory only
- a reserved virtual layout for kernel and future user spaces
- active x86_64 page-table mutation for kernel heap mapping
- a bootstrap bump allocator for kernel heap allocations

## Physical memory policy

The boot memory map is captured after `ExitBootServices` and normalized into
`BootMemoryRegion` entries.

Region handling:

- `CONVENTIONAL` memory becomes `Usable`
- `BOOT_SERVICES_CODE` and `BOOT_SERVICES_DATA` become
  `BootServicesReclaimable`
- loader image memory remains reserved
- ACPI reclaimable memory is tracked but not reused yet
- MMIO and firmware/runtime ranges remain reserved

The current frame allocator only allocates from `Usable` regions. This is
deliberately conservative so the kernel does not trample firmware-owned data
while it still runs on the firmware’s active page tables.

## Early frame allocator

The Phase 1 frame allocator is region-based and monotonic.

Invariants:

- frames are 4 KiB aligned
- only `Usable` regions enter the allocator
- allocations are unique and never reused in Phase 1
- the allocator never hands out reclaimable firmware memory yet

This is sufficient for:

- page-table growth while mapping new kernel pages
- heap backing pages
- future short-term bootstrap structures

## Virtual memory layout

Phase 1 reserves the following conceptual layout:

- lower canonical half: future user address spaces
- `0xffff_8000_0000_0000..0xffff_c000_0000_0000`: reserved future physical
  window
- `0xffff_c100_0000_0000..0xffff_c100_0020_0000`: mapped kernel heap
- `0xffff_c200_0000_0000..0xffff_c201_0000_0000`: reserved future kernel
  object arena

Only the heap range is actively mapped in Phase 1.

## Paging strategy

The kernel currently reuses the active firmware page tables and wraps them with
an x86_64 mapper.

Phase 1 assumptions:

- physical memory is reachable through the firmware’s flat mapping
- the active CR3 root remains valid after exiting boot services
- page-table writes require temporarily clearing `CR0.WP`

This is a bring-up strategy, not the final VM design.

## Kernel heap

The heap is backed by real mapped pages from the early frame allocator.

Allocator properties:

- simple bump allocator
- thread-safe through a spin mutex
- deallocation is intentionally minimal and only resets the bump pointer once
  all live allocations are gone

That keeps unsafe code small while giving later phases a stable global
allocation entry point.

## Next steps this enables

- reclaim boot-services memory safely
- install kernel-owned top-level page tables
- add a direct physical-memory window
- create real user address spaces with shared kernel mappings
- replace the bootstrap heap with longer-lived slab/object allocators
