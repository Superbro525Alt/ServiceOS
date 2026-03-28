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
- a persisted manifest and resource source in the boot store
- dependency-aware startup
- capability-scoped service startup grants
- manager-mediated service registration and discovery
- basic restart supervision

Later work should extend the root manager with:

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

The first networking platform layer now exists in userspace. That is the
intended direction: networking policy stays outside the kernel except for
low-level interrupt, memory, and packet-object mechanisms.

The current platform already has:

- a kernel packet-interface object with explicit capability rights
- a userspace `network-service` that owns interface state and IPv4 policy
- a generic service contract for interface status, static route reporting, host
  resolution, and ICMP probes
- a VirtIO PCI bring-up path that sits behind the packet-interface boundary
  rather than defining it

Deferred work:

- richer NIC interrupt models such as MSI/MSI-X and broader multi-device
  routing
- additional virtual backends and real NIC driver hosts
- DHCP, DNS, richer routing, and socket-like protocol services
- packet-buffer sharing and zero-copy policy beyond the current copied-frame
  path

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

The first graphics/session platform layer now exists in userspace. That is the
intended direction: display and session policy stay outside the kernel except
for low-level output objects and trap/memory mechanisms.

The current platform already has:

- boot framebuffer discovery in `BootContext`
- a kernel display-output object with explicit rights
- a `graphics-service` that owns outputs, surfaces, and composition
- a `session-service` that owns graphical session identity and focus policy
- shell/operator commands that inspect the live graphics/session state through
  the real service contracts

Deferred work:

- shared-memory presentation buffers and richer client rendering protocols
- physical input-device hosts and routing policy
- multiple outputs and multiple sessions
- richer window-management, notification, and desktop-shell policy beyond the
  current first shell

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
- a boot-store based executable bootstrap path that can evolve into richer
  loaders later

Deferred work:

- richer memory-mapping syscalls
- richer fault upcalls to user processes beyond terminate-on-fault isolation
- executable-loader expansion beyond the boot-store bootstrap format

## Shell and session stack

A minimal text shell and a first graphical desktop shell now exist as normal
userspace services. Later shell, session-manager, and application runtime work
should build on those userspace boundaries instead of pulling terminal or
desktop policy back into the kernel.

Deferred work:

- multiple sessions and login policy
- package-backed command discovery
- richer terminal semantics and networking tools
- polished desktop UX, notifications, and richer app/session policy
- broader graphical app toolkit and runtime layers
