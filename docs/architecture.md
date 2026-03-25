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
low-level boot, memory, and control-flow substrate they will eventually
require.

## What the kernel owns now

- firmware handoff normalization
- physical memory discovery
- conservative early frame allocation
- x86_64 page-table mutation for kernel-owned mappings
- bootstrap heap allocation
- kernel address-space root tracking
- x86_64 descriptor tables and trap entry
- timer interrupt delivery and monotonic tick accounting
- deadline wakeup bookkeeping for future blocking operations
- syscall dispatch structure without high-level policy

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
- the x86_64 trap path now uses the nightly `extern "x86-interrupt"` ABI for
  exceptions and IRQ handlers, while the syscall vector keeps a small assembly
  shim for general-purpose register capture

## Temporary but explicit assumptions

- The early x86_64 bring-up uses the firmware’s flat physical mapping with
  `physical_memory_offset = 0`
- Page-table writes are bracketed by temporarily clearing `CR0.WP` because the
  firmware keeps its active page-table pages read-only
- Only UEFI `CONVENTIONAL` memory is handed to the early frame allocator
- Legacy PIC + PIT are used as the first timer source because they are simple,
  deterministic, and work well under QEMU bring-up
- Syscalls use interrupt vector `0x80` as the initial entry point instead of a
  faster `SYSCALL/SYSRET` path because user-mode stacks and privilege
  transitions are not built yet

These are Phase 2 bring-up constraints, not long-term ABI commitments.
