# Kernel Architecture

## Philosophy

The kernel provides mechanisms, not policy.

The intended long-term structure remains:

```text
kernel
  -> root service manager
    -> foundational services
      -> platform services
        -> shells, runtimes, applications, compatibility layers
```

Phase 1 still does not implement those service layers. It only establishes the
low-level boot and memory substrate they will eventually require.

## What the kernel owns now

- firmware handoff normalization
- physical memory discovery
- conservative early frame allocation
- x86_64 page-table mutation for kernel-owned mappings
- bootstrap heap allocation
- kernel address-space root tracking

## What stays out of the kernel for now

- service startup policy
- driver policy
- filesystem and storage semantics
- networking policy
- application/runtime policy
- desktop and graphics policy

## Current architectural choices

- `x86_64` + UEFI + QEMU is the primary bring-up path
- generic kernel code owns abstract memory and address-space concepts
- the x86_64 crate owns UEFI boot parsing and page-table mutation details
- the active firmware page tables are reused during Phase 1 instead of being
  replaced immediately
- the kernel reserves a future high-half layout even though only the heap is
  actively mapped there today

## Temporary but explicit assumptions

- The early x86_64 bring-up uses the firmware’s flat physical mapping with
  `physical_memory_offset = 0`
- Page-table writes are bracketed by temporarily clearing `CR0.WP` because the
  firmware keeps its active page-table pages read-only
- Only UEFI `CONVENTIONAL` memory is handed to the early frame allocator

These are Phase 1 bring-up constraints, not long-term ABI commitments.
