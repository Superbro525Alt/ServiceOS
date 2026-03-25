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

Still owns identifiers and future execution-container concepts, but it does not
yet create runnable processes because the memory model is only now being
established.

## `ipc`

Still deferred. No policy-heavy IPC implementation belongs here until address
spaces and syscall entry are stable.

## `capability`

Still defines authority concepts only. Real capability-space population waits
for object and address-space infrastructure.

## `object`

Still defines kernel object identity and typing. Later phases will pair it with
real allocation and lifetime management.

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
