# Memory Foundation

## What exists now

- UEFI memory-map normalization into architecture-neutral boot regions
- early frame allocation from `CONVENTIONAL` memory plus explicit
  post-bootstrap boot-services reclaim
- a reserved virtual layout for kernel and future user spaces
- active x86_64 page-table mutation for kernel heap mapping
- a reusable free-list kernel heap allocator
- dedicated owned page-table roots for user address spaces
- flat-image mapping for bootstrap user code regions and user stacks
- memory-object info and range-based mapping syscalls for userspace

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

The kernel first allocates from `Usable` regions during the paging and heap
bootstrap, then explicitly folds `BootServicesReclaimable` regions into the
frame allocator once the kernel is fully past firmware ownership. That keeps
the reclaim point explicit instead of silently treating firmware memory as free
from the start.

## Early frame allocator

The frame allocator is region-based and monotonic.

Invariants:

- frames are 4 KiB aligned
- `Usable` regions enter first
- boot-services regions are added only after the initial kernel heap is mapped
- allocations are unique and never reused
- ACPI reclaimable memory is still tracked but not reused

## Virtual memory layout

The current conceptual layout is:

- lower canonical half: future user address spaces
- `0xffff_8000_0000_0000..0xffff_c000_0000_0000`: reserved future physical
  window
- `0xffff_c100_0000_0000..0xffff_c100_0200_0000`: mapped kernel heap
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

- reusable free-list allocator
- thread-safe through a spin mutex
- coalesces adjacent free regions
- still serves as a general bootstrap heap, not a final slab/object allocator

## Next steps this enables

The kernel now exposes a broader memory-object syscall surface than the
original create/read/write/full-map baseline:

- memory-object info queries
- range-based memory-object mapping with offset/length selection
- rights-aware writable mapping checks

Those syscalls are still deliberately narrower than a final VM API:

- mappings are still reserved from the runtime-managed shared range instead of
  a fully general per-process VM allocator
- fixed-address mappings, unmap, protect, and full virtual-memory queries
  remain deferred
- the kernel still reuses the boot-established top-level page tables instead of
  owning the entire kernel page-table lifecycle

- install fully kernel-owned top-level page tables
- add a direct physical-memory window
- extend the current memory-object mapping API into fuller VM construction,
  unmap, and protection controls
- layer shared-memory IPC transports on top of the now-generic memory-object
  mapping substrate
- layer dedicated slab/object allocators on top of the general kernel heap
