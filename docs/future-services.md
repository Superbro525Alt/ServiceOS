# Future Service Readiness

The kernel and root bootstrap layer are now prepared to host a broader
service-oriented system, but they are not implementing those services yet. This
document records the intended integration boundary so later phases do not pull
policy back into the kernel.

## Root service manager

A minimal root service manager now exists and launches as the first userspace
task. The next phases should refine it, not move its policy back into the
kernel.

The current system already has:

- a kernel-to-root bootstrap path
- a built-in service manifest catalog
- dependency-aware startup
- capability-scoped service startup grants
- manager-mediated service registration and discovery
- basic restart supervision

Later work should extend the root manager with:

- an explicit bootstrap capability object instead of the temporary root-role
  gate
- persistent manifest sources and trust policy
- richer supervision and health policy
- fault-aware process isolation and recovery

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
- executable-loader expansion beyond the built-in flat-image catalog

## Desktop shell

The future shell, session manager, and application runtime stack should sit
above the root service manager and platform services. Nothing in the current
kernel hardcodes desktop concepts, which is the correct long-term boundary.
