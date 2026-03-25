# Subsystem Boundaries

## `bootstrap`

Owns firmware handoff normalization and the transition into generic kernel
initialization.

## `memory`

Owns:

- physical memory classification
- early frame allocation
- virtual layout definitions
- heap bootstrap
- address-space root descriptions

It does not own scheduler policy, service policy, or userspace loading.

## `task`

Now owns:

- task and thread object state
- task roles and future-ready address-space attachment points
- execution-state and wait-target bookkeeping for later scheduling
- per-task capability-space ownership

It still does not own scheduling policy or user-mode execution.

## `ipc`

Now owns:

- channel endpoint objects
- bounded message shape rules
- capability-carrying messages
- send and receive semantics independent of service policy

It still does not own RPC policy, broker policy, or shared-memory protocol
semantics.

## `capability`

Now owns:

- handle allocation within a capability space
- per-handle rights masks
- duplication and transfer checks
- explicit close semantics

It does not own naming policy or global discovery.

## `object`

Now owns:

- registry-backed object identity
- the unified object taxonomy used by task, IPC, timer, and memory subsystems
- weak-indexed live-object tracking
- bootstrap root-task creation

It still does not own slab caches or long-term allocator specialization.

## `syscall`

Now owns:

- syscall number typing
- a small dispatch table
- explicit return/error encoding
- the long-term boundary between trap entry and object/capability policy

It still does not own process ABI policy, handle tables, or user-buffer
marshalling.

## `interrupts`

Now owns:

- generic trap classification
- fault disposition decisions
- interrupt/syscall accounting
- the boundary between arch-specific trap entry and generic kernel policy

The x86_64 crate owns descriptor tables, IRQ acknowledgement, and low-level
entry details. The core crate owns classification and counting.

## `time`

Now owns:

- monotonic tick accounting
- timer-source description
- deadline registration
- ready-to-wake token queues for later schedulers and IPC waits

It still does not own a scheduler, CPU accounting, or wall-clock policy.
