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

The current code still does not implement those service layers. It establishes
the low-level boot, memory, control-flow, object, and execution substrate they
will eventually require.

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
- registry-backed kernel object identity and typing
- per-task capability spaces as the authority boundary
- channel IPC and capability transfer primitives
- object lifetime tracking through handles plus strong object references
- process-equivalent task objects with address-space attachment points
- a bootstrap kernel thread plus schedulable service threads
- a simple round-robin scheduler with timer and IPC wake integration
- dedicated user address-space roots for user threads
- a bootstrap userspace loader and ring-3 transition path
- a minimal syscall ABI sufficient for the first user program

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
- generic kernel code also owns objects, capabilities, IPC semantics, and
  lifetime rules
- the x86_64 crate owns UEFI boot parsing and page-table mutation details
- the active firmware page tables are reused during Phase 1 instead of being
  replaced immediately
- the kernel reserves a future high-half layout even though only the heap is
  actively mapped there today
- the x86_64 trap path now uses the nightly `extern "x86-interrupt"` ABI for
  exceptions and IRQ handlers, while the syscall vector keeps a small assembly
  shim for general-purpose register capture
- each task object owns its own capability space, so later userspace services
  can be composed around explicit handle transfer instead of global namespaces
- the current task object is the kernel's process-equivalent abstraction; later
  userspace service processes will refine this with real user address spaces
- the scheduler remains intentionally simple, but its blocking model is already
  aligned with timer waits, channel receives, and future syscall blocking
- the first user executable format is a kernel-owned flat image because Phase 5
  is about launch mechanics, not about baking ELF policy into the kernel too
  early
- the first user syscall ABI stays on interrupt vector `0x80`; the priority is
  a clean privilege boundary before any fast-path syscall work

## Temporary but explicit assumptions

- The early x86_64 bring-up uses the firmware’s flat physical mapping with
  `physical_memory_offset = 0`
- Page-table writes are bracketed by temporarily clearing `CR0.WP` because the
  firmware keeps its active page-table pages read-only
- Only UEFI `CONVENTIONAL` memory is handed to the early frame allocator
- Legacy PIC + PIT are used as the first timer source because they are simple,
  deterministic, and work well under QEMU bring-up
- Syscalls use interrupt vector `0x80` as the initial entry point instead of a
  faster `SYSCALL/SYSRET` path because the kernel is still proving out the
  first user ABI and wants the most explicit control-flow path
- The kernel object registry currently uses `Arc` ownership and a weak-indexed
  live-object table because it is simple, explicit, and a good fit for early
  Rust bring-up
- Channels are the first IPC primitive because they align with the long-term
  service graph and keep authority transfer explicit
- A single-core round-robin scheduler is the first execution policy because it
  is easy to reason about and leaves room for later preemption and SMP work

These are Phase 5 bring-up constraints, not long-term ABI commitments.
