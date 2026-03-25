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

The current code now implements the first userspace layer of that structure:
the root service manager plus a very small bootstrap service graph. The broader
platform and application layers remain deferred.

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
- the minimal syscall ABI needed for the root service manager and child services

## What lives in userspace now

- service manifests
- service startup ordering
- service registration and discovery policy
- capability distribution from root into child services
- restart and failure handling policy
- service lifecycle logging

## What stays out of the kernel for now

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
- the active firmware page tables are reused during early bring-up instead of
  being replaced immediately
- the kernel reserves a future high-half layout even though only the heap is
  actively mapped there today
- the x86_64 trap path uses the nightly `extern "x86-interrupt"` ABI for
  exceptions and IRQ handlers, while the syscall vector keeps a small assembly
  shim for general-purpose register capture
- each task object owns its own capability space, so userspace services are
  composed around explicit handle transfer instead of global namespaces
- the current task object is the kernel's process-equivalent abstraction; later
  userspace service processes will refine this with richer address-space policy
- the scheduler remains intentionally simple, but its blocking model is aligned
  with timer waits, channel receives, and future syscall blocking
- the current user executable format is a kernel-owned flat image because this
  stage is about launch mechanics and service bootstrap, not storage policy
- service manifests, resources, and executable images currently come from a
  small staged boot store so the platform can prove persisted startup without
  pulling full filesystem or package policy into the kernel
- discovery is manager-mediated rather than kernel-global so service composition
  stays capability-oriented
- the first user syscall ABI stays on interrupt vector `0x80`; the priority is
  a clean privilege boundary before any fast-path syscall work

## Temporary but explicit assumptions

- the early x86_64 bring-up uses the firmware’s flat physical mapping with
  `physical_memory_offset = 0`
- page-table writes are bracketed by temporarily clearing `CR0.WP` because the
  firmware keeps its active page-table pages read-only
- only UEFI `CONVENTIONAL` memory is handed to the early frame allocator
- legacy PIC + PIT are used as the first timer source because they are simple,
  deterministic, and work well under QEMU bring-up
- syscalls use interrupt vector `0x80` as the initial entry point instead of a
  faster `SYSCALL/SYSRET` path because the kernel is still proving out the
  first user ABI and wants the most explicit control-flow path
- the kernel object registry currently uses `Arc` ownership and a weak-indexed
  live-object table because it is simple, explicit, and a good fit for early
  Rust bring-up
- channels are the first IPC primitive because they align with the long-term
  service graph and keep authority transfer explicit
- the bootstrap root still uses a temporary kernel role gate for service spawn;
  later work should replace that with an explicit bootstrap capability object
- a single-core round-robin scheduler is the first execution policy because it
  is easy to reason about and leaves room for later preemption and SMP work

These are early bring-up constraints, not long-term ABI commitments.
