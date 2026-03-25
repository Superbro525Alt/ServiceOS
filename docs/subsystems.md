# Subsystem Boundaries

## `bootstrap`

Boot-time normalization and kernel entry coordination. This module owns the
architecture-neutral view of the boot handoff and defines what later phases can
rely on after early bring-up completes.

## `memory`

Owns physical memory discovery, frame allocation, kernel virtual memory layout,
and later VM object plumbing. It should expose mechanisms, not policy-driven
allocation behavior for services.

## `task`

Owns address spaces, tasks, threads, and scheduling context handles. Policy such
as service startup ordering belongs outside this module.

## `ipc`

Owns kernel-mediated message transport primitives that services will use to talk
to each other. Naming, routing policy, and service discovery stay out of the
kernel.

## `capability`

Owns access rights, transfer semantics, and capability-space interfaces. It is
the kernel’s authority model.

## `object`

Owns the base identity and typing model for kernel-managed resources. Other
subsystems build on this instead of inventing private object registries.

## `syscall`

Owns the user-kernel entry contract and dispatch boundaries. It should translate
calls into kernel mechanisms without embedding high-level service policy.

## `interrupts`

Owns architecture-neutral interrupt and exception concepts. Actual descriptor
tables and controller programming stay in the architecture layer.

## `time`

Owns monotonic time, timer abstractions, and later deadline primitives. Clock
selection policy remains architecture-specific until more hardware support
exists.

