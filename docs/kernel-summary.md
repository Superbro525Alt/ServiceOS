# Kernel Summary

This repository now covers a complete early-kernel foundation plus the first
durable userspace platform layer.

## Design stance

- the kernel provides mechanisms, not service policy
- authority flows through handles in per-task capability spaces
- communication flows through explicit IPC objects, not ambient global access
- architecture-specific code stays in `kernel/arch/*`; generic policy-neutral
  code stays in `kernel/core`
- unsafe code is concentrated around firmware handoff, page-table mutation,
  interrupt/syscall entry, and userspace transition

## Boot and init

The boot path is:

1. firmware enters the `x86_64` UEFI image
2. architecture code captures the firmware memory map and exits boot services
3. generic kernel initialization builds memory, interrupt, syscall, time,
   object, IPC, and task foundations
4. architecture code finishes descriptor-table and timer bring-up
5. the kernel creates the root userspace address space, loads the root-manager
   image, and enters ring 3
6. the root manager starts the foundational service graph

## Kernel mechanisms now in place

- physical memory discovery and early frame allocation
- kernel virtual layout and bootstrap heap
- GDT, TSS, IDT, PIC, and PIT bring-up on `x86_64`
- monotonic time and deadline wakeups
- task objects, thread objects, and a simple scheduler
- handle tables with duplication, transfer, and close semantics
- channel IPC with bounded queues
- user address-space construction and ring-3 entry
- a small syscall ABI for service composition

## What moved to userspace

The first true policy now lives outside the kernel:

- service startup ordering
- service registration and lookup policy
- restart and supervision policy
- structured logging
- shared configuration
- console-adjacent output routing

That is the correct architectural direction for the project.

## Current foundational services

- `console-service`: owns the immediate route to the debug output sink
- `config-service`: serves a small typed configuration schema
- `log-service`: filters and forwards structured logs
- `status-service`: first dependent long-running platform service

## Deferred responsibilities

The kernel is intentionally not doing the following yet:

- executable discovery or package policy
- filesystems
- networking stacks
- audio pipelines
- graphics/compositor policy
- desktop shell behavior
- compatibility runtimes

Those remain later userspace responsibilities.
