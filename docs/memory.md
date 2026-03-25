# Memory Foundation

## What exists now

- UEFI memory-map normalization into architecture-neutral boot regions
- early frame allocation from `CONVENTIONAL` memory only
- a reserved virtual layout for kernel and future user spaces
- active x86_64 page-table mutation for kernel heap mapping
- a bootstrap bump allocator for kernel heap allocations
- dedicated owned page-table roots for user address spaces
- flat-image mapping for bootstrap user code regions and user stacks

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
deliberately conservative while the kernel still runs on the firmware’s active
page tables.

## Early frame allocator

The frame allocator is region-based and monotonic.

Invariants:

- frames are 4 KiB aligned
- only `Usable` regions enter the allocator
- allocations are unique and never reused
- reclaimable firmware memory is not allocated yet

## Virtual memory layout

The current conceptual layout is:

- lower canonical half: future user address spaces
- `0xffff_8000_0000_0000..0xffff_c000_0000_0000`: reserved future physical
  window
- `0xffff_c100_0000_0000..0xffff_c100_0020_0000`: mapped kernel heap
- `0xffff_c200_0000_0000..0xffff_c201_0000_0000`: reserved future kernel
  object arena

Only the heap range is actively mapped today.

## Paging strategy

The kernel currently reuses the active firmware page tables and wraps them with
an x86_64 mapper.

Current bring-up assumptions:

- physical memory is reachable through the firmware’s flat mapping
- the active CR3 root remains valid after exiting boot services
- page-table writes require temporarily clearing `CR0.WP`

This is a bring-up strategy, not the final VM design.

## Kernel heap

The heap is backed by real mapped pages from the early frame allocator.

Allocator properties:

- simple bump allocator
- thread-safe through a spin mutex
- deallocation is intentionally minimal

## Next steps this enables

- reclaim boot-services memory safely
- install fully kernel-owned top-level page tables
- add a direct physical-memory window
- expose richer VM construction and mapping APIs to later process code
- replace the bootstrap heap with longer-lived slab or object allocators
