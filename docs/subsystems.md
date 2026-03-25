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

Still deferred past memory and address-space bring-up. The syscall layer should
land only after user/kernel memory boundaries are explicit.

## `interrupts`

Still architecture-backed and intentionally light. Descriptor tables and fault
paths come after the memory foundation is stable.

## `time`

Still reserved for later timer and deadline work once interrupt delivery is in
place.
