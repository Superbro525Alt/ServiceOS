# Future Service Readiness

This kernel is now prepared to host a future service-oriented system, but it is
not implementing those services yet. This document records the intended
integration boundary so later phases do not pull policy back into the kernel.

## Root service manager

The future root service manager should launch as a normal userspace task with a
privileged initial capability set. The kernel is already prepared for that
direction through:

- per-task capability spaces
- schedulable user threads
- user address-space bootstrap
- channel IPC with handle transfer
- timer-driven blocking and wakeups

The root manager should eventually own:

- service launch ordering
- naming and discovery policy
- initial capability distribution
- restart and fault-recovery policy

## Filesystem services

The kernel does not define a VFS or pathname policy today. That is intentional.
Future filesystem services can sit on top of:

- memory objects for future page-cache and shared-buffer work
- channels for request/reply control flow
- task isolation for driver or filesystem sandboxing

What remains deferred:

- block-device service contracts
- file and directory object protocols
- mount policy and namespace composition

## Networking services

Networking should remain outside the kernel except for low-level interrupt and
memory mechanisms.

The current kernel is already suitable for:

- dedicated network driver hosts
- packet-service daemons
- capability-scoped access to NIC-facing objects later
- timer-based retransmit and timeout logic in userspace

Deferred work:

- NIC object model
- packet buffer sharing strategy
- socket-like userspace protocols

## Audio services

Audio is deferred to later userspace services. The kernel already provides the
pieces needed for that direction:

- isolated tasks for mixers and device hosts
- IPC channels for control and event flow
- timers for buffer deadlines and wakeups

Deferred work:

- DMA-safe memory-object policy
- device event objects for audio engines
- stream graph and policy design

## Graphics and compositor

The kernel is not a window system. For later graphics work it currently offers:

- framebuffer discovery in `BootContext` for very early platform handoff
- user task isolation
- syscall and IPC paths suitable for a compositor/service split

Deferred work:

- display device objects
- shared-memory presentation buffers
- input routing and compositor protocols

## Package and update systems

Package management, rollback, and update policy remain entirely outside the
kernel. The kernel should only supply:

- isolation boundaries
- object rights
- IPC transport

Later services should own:

- package trust policy
- storage layout
- upgrade orchestration
- rollback semantics

## Compatibility runtimes

Compatibility layers should be hosted as userspace runtimes or subsystem
services, not welded into the kernel.

The current kernel already supports that direction through:

- separate tasks and user threads
- explicit syscall growth rather than ambient host ABI leakage
- channel-based service boundaries

Deferred work:

- richer memory-mapping syscalls
- fault delivery to user processes
- executable-loader expansion beyond the flat bootstrap image

## Desktop shell

The future shell, session manager, and application runtime stack should sit
above the root service manager and platform services. Nothing in the current
kernel hardcodes desktop concepts, which is the correct long-term boundary.
