# Kernel Summary

This repository now covers a complete early-kernel foundation through Phase 6.
The code is still intentionally small, but the boundaries are now shaped for a
long-lived service-oriented system rather than a one-off hobby kernel.

## Design stance

- the kernel provides mechanisms, not service policy
- authority flows through handles in per-task capability spaces
- communication flows through explicit IPC objects, not ambient global access
- architecture-specific code stays in `kernel/arch/*`; generic policy-neutral
  code stays in `kernel/core`
- unsafe code is concentrated around firmware handoff, page-table mutation,
  interrupt/syscall entry, and userspace transition

## Boot and init

The current boot path is:

1. firmware enters the `x86_64` UEFI image
2. architecture code captures the firmware memory map and exits boot services
3. generic kernel initialization builds the memory manager, interrupt state,
   syscall dispatcher, time manager, object model, IPC kernel, and task system
4. architecture code finishes descriptor-table and timer bring-up
5. the kernel creates a first user address space, loads the demo image, and
   enters ring 3

The `BootContext` is the contract between firmware handoff and generic kernel
initialization. It carries normalized memory-region data plus optional platform
discovery details such as the RSDP and framebuffer.

## Memory model

The memory subsystem is still early, but it is real.

- physical memory is seeded from the UEFI memory map
- only `Usable` regions are fed into the early frame allocator
- reclaimable firmware regions are tracked but not yet handed back to the
  general allocator
- the kernel virtual layout reserves a dedicated heap window
- the current heap allocator is a simple bump allocator suitable for bootstrap
  object creation and internal metadata
- x86_64 page-table mutation is isolated in the arch crate and exposed through
  generic mapping traits

The current design already distinguishes:

- physical frames
- the kernel address-space root
- future user address-space layout
- mapping flags and intended memory use

That is enough to evolve toward per-process address spaces without rewriting the
core abstractions.

## Interrupts, exceptions, syscalls, and time

The trap path is intentionally explicit.

- x86_64 installs a GDT, TSS, and IDT
- hardware IRQs are remapped through the PIC
- the PIT provides the current monotonic tick source
- timer expirations are translated into wake events consumed by the scheduler
- syscall entry uses a dedicated software-interrupt vector and a narrow ABI
  surface

The current syscall ABI is minimal on purpose:

- ABI version probe
- monotonic time read
- current-thread exit

The dispatch path is table-driven and easy to grow, but the kernel does not yet
pretend to expose a complete user API.

## Execution model

The execution model now has a real shape.

- task objects are the current process-equivalent container
- each task owns a capability space and may later own a user address space
- thread objects carry execution mode, register-entry metadata, wait target, and
  last wake reason
- the scheduler is deliberately simple round-robin state machine code
- blocking integrates with channel receive and timer deadlines

The current scheduler is not sophisticated. It is designed to be inspectable and
correct enough that later preemption, CPU-local queues, and policy layers can be
added without rewriting the thread/task model.

## Object, capability, and IPC model

The kernel object model is unified around the registry in `kernel/core/src/object`.
Current first-class object kinds are:

- task
- thread
- channel endpoint
- event
- timer
- memory object

Handles are the sole supported authority path to these objects.

- every handle names exactly one object
- every handle carries an explicit rights mask
- duplication and transfer can only reduce rights
- moving a handle removes it from the sender space
- closing a handle drops one strong reference to the target object

IPC is channel-based and kernel-mediated.

- a channel is represented as a connected pair of endpoint objects
- messages carry a small word payload
- messages may transfer a bounded set of rights-reduced handles
- reply endpoints are explicit and must themselves refer to channel endpoints
- per-endpoint queues are bounded to keep failure modes explicit

This is the substrate for a future service graph. The kernel is not acting as an
RPC framework, service registry, or policy engine.

## Userspace bootstrap

The current userspace path proves that the kernel can hand control outward.

- a dedicated user page-table root is created with shared kernel mappings
- a minimal flat executable image is parsed and mapped
- a bootstrap user stack is created
- a user thread enters ring 3 and uses the syscall ABI
- user-thread exit returns control to the kernel cleanly

The demo userspace program is intentionally tiny. Its purpose is validation of
the kernel/userspace boundary, not early service-manager policy.

## Hardening and tests

Phase 6 tightened several contracts that were previously only implicit.

- capability-handle exhaustion is now reported instead of silently saturating
- IPC reply-endpoint and queue-capacity validation is enforced
- duplicate thread attachment within a task is suppressed
- scheduler wake-token exhaustion is explicit
- host-side unit tests no longer depend on the freestanding kernel allocator

The unit-test suite now exercises:

- capability duplication, transfer, and exhaustion rules
- IPC rights transfer, reply-endpoint validation, and queue bounds
- object lifetime cleanup
- frame-allocator invariants
- syscall dispatch validation
- scheduler block/wake transitions
- user-image parsing rules

## Deferred responsibilities

The kernel is intentionally not doing the following yet:

- root service-manager policy
- executable discovery or package policy
- filesystems
- networking stacks
- audio pipelines
- graphics/compositor policy
- desktop shell behavior
- compatibility runtimes

Those belong in later userspace services. The kernel is now prepared to support
them through address spaces, handles, IPC, timers, and schedulable threads.
