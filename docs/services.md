# Platform and Product Services

## Current graph

The current always-on userspace graph is:

```text
root-manager
  -> storage-service
  -> console-service
  -> config-service
       depends on storage-backed config resource
  -> log-service
       depends on console-service, config-service
  -> network-service
       depends on log-service, config-service
       consumes one startup-granted packet-interface capability
       consumes one startup-granted hosts resource
  -> graphics-service
       depends on log-service
       consumes one startup-granted display-output capability
  -> session-service
       depends on graphics-service, log-service
  -> desktop-shell-service
       depends on graphics-service, session-service, log-service,
       network-service, status-service
  -> status-service
       depends on log-service, config-service
  -> package-service
       depends on storage-service, log-service
  -> shell-service
       depends on console-service, log-service, config-service,
       storage-service, network-service, graphics-service,
       session-service, desktop-shell-service, status-service,
       package-service
```

The current repository-backed optional package graph is:

```text
package-service
  -> announce-service package
       versioned at 1.0.0 and 1.1.0
       activated on operator request
```

The current transient graphical app graph is:

```text
desktop-shell-service
  -> monitor-app
       launched automatically on desktop bring-up
  -> settings-app
       launched on desktop request
  -> files-app
       launched on desktop request
```

The important change is that the graph now consumes persisted inputs. The root
manager loads service manifests from `storage-service`, `config-service` reads a
real config blob, and `status-service` reads a startup-granted resource blob.

## Root manager responsibilities

The root manager is the first real system coordinator in userspace. It owns:

- boot-root bootstrap sequencing
- starting `storage-service` from the kernel-provided boot-store capability
- loading the service index and manifests from storage
- dependency ordering
- startup capability grants
- service registration and lookup mediation
- restart supervision
- shell-facing service inspection and transient tool launch
- execution of dynamic service activation and deactivation requests from
  `package-service`

The kernel still only provides mechanisms: address spaces, threads, channels,
capabilities, timers, and the executable launch path.

## Capability distribution

The system does not reintroduce ambient authority through storage.

- The kernel gives the root manager one read-only boot-store capability.
- The root manager passes that capability only to `storage-service`.
- Other services do not get the storage root by default.
- The root manager opens specific resources through `storage-service` and
  transfers only those resource/blob capabilities to children that need them.
- Service-to-service communication remains manager-mediated and rights-reduced.
- Display ownership follows the same pattern: only `graphics-service` receives
  the bootstrap display-output capability.

Current startup grants:

- `storage-service`
  - boot-store memory object capability from the kernel/root bootstrap
- `config-service`
  - one blob capability for `config/system.cfg`
- `log-service`
  - send-only handle to `console-service`
  - send-only handle to `config-service`
- `status-service`
  - send-only handle to `log-service`
  - one blob capability for `services/status-service/resources/banner.txt`
- `network-service`
  - one packet-interface capability from the root bootstrap path
  - send-only handle to `log-service`
  - one blob capability for `config/hosts.cfg`
- `graphics-service`
  - one display-output capability from the root bootstrap path
  - send-only handle to `log-service`

Current lookup permissions:

- `status-service`
  - `config-service` with send-only rights
  - `console-service` with send-only rights
- `shell-service`
  - `console-service` with send-only rights
  - `log-service` with send-only rights
  - `config-service` with send-only rights
  - `storage-service` with send-only rights
  - `status-service` with send-only rights
  - `package-service` with send-only rights
  - `network-service` with send-only rights
  - `graphics-service` with send-only rights
  - `session-service` with send-only rights
  - `desktop-shell-service` with send-only rights
- `package-service`
  - `storage-service` with send-only rights
- `session-service`
  - `graphics-service` with send-only rights
- `desktop-shell-service`
  - `graphics-service` with send-only rights
  - `session-service` with send-only rights
  - `network-service` with send-only rights
  - `status-service` with send-only rights

## Service roles

### `storage-service`

- mounts the immutable boot store handed off from firmware through the kernel
- exposes an exact-path open contract to the root manager
- turns persisted files into explicit blob capabilities
- establishes the storage/resource capability pattern without exposing a global
  filesystem namespace

### `console-service`

- owns the userspace route to the kernel debug sink
- renders structured service and lifecycle events into readable diagnostics
- owns line-oriented operator sessions backed by the raw serial console path

### `config-service`

- reads typed configuration values from a persisted config blob
- exposes a stable request/reply contract for shared system configuration

### `log-service`

- is the durable userspace log sink
- filters records by configured minimum severity
- forwards readable output through `console-service`

### `status-service`

- is the first long-running dependent platform service
- proves both manager-mediated lookup and startup-granted resource access
- reads config, consumes a resource blob, and emits periodic heartbeats

### `network-service`

- owns interface state and IP-level networking policy in userspace
- consumes an explicit packet-interface capability rather than ambient NIC
  access
- applies static IPv4 configuration from `config-service`
- loads static host mappings from a storage-backed resource blob
- exposes interface status, route reporting, host resolution, and ICMP probe
  requests through a stable service contract
- keeps the public contract generic so later VirtIO, additional virtual, and
  real-NIC backends can sit behind the same service boundary

### `graphics-service`

- owns display-output state in userspace
- consumes the explicit kernel display-output capability
- creates and tracks surfaces through per-surface handles
- composes the current surface set into the active output
- keeps compositor policy in userspace while leaving the kernel with only
  display-output mechanism

### `session-service`

- owns graphical session identity and focus policy
- looks up `graphics-service` rather than owning display hardware itself
- proves the split between compositor mechanics and session/input policy
- provides the initial basis for later login, desktop shell, and multi-session
  work

### `desktop-shell-service`

- is the first graphical product shell built on top of the platform services
- owns desktop chrome, launcher state, retained window state, and app focus
  tracking
- creates shell-owned surfaces through `graphics-service`
- retains the authoritative surface handle for each app window
- creates one app-control channel per launched app for focus, resize, and close
  delivery
- owns move, resize, minimize, restore, and close policy for desktop windows
- accepts desktop interaction requests for hit testing and pointer-style actions
- asks the root manager to launch graphical apps instead of spawning tasks
  directly
- keeps desktop product policy out of `graphics-service` and `session-service`

### `settings-app`, `files-app`, and `monitor-app`

- are transient graphical applications rather than long-running platform
  services
- receive one surface handle, one app-control channel, and a small explicit
  service-handle set
- validate the current platform contracts for config, storage, status, and
  network access
- stay replaceable and non-ambient: they do not inherit manager or compositor
  authority

### `shell-service`

- owns the first operator/developer command environment
- opens a console session through `console-service`
- inspects services through the manager control channel
- reads logs, config, and storage through explicit service lookups
- inspects outputs, surfaces, and sessions through `graphics-service` and
  `session-service`
- launches transient tools through the manager rather than direct shell power

### `package-service`

- owns package repository inspection and operator-facing install/update/remove
  policy
- decides which package manifest version should become active and when rollback
  should be attempted
- reads repository metadata and package manifests from `storage-service`
- calls back into the root manager for package-provided service activation and
  deactivation
- keeps package authority explicit by requiring an explicit service handle; the
  shell has it, ordinary services do not

### `announce-service`

- is the first package-provided long-running service
- consumes a package-owned resource blob through the normal service-manifest
  resource path
- proves package activation, version switch, removal, and rollback without
  moving package policy into the kernel

## Registry and discovery

The registry remains manager-mediated and identity-based.

- services register under a stable `ServiceId`
- the manager retains the registered public handle
- lookups go through the caller's bootstrap/control channel
- lookup policy is derived from the caller's manifest
- replies carry a newly duplicated rights-reduced handle

Discovery is explicit. Knowing a service name does not imply access.

The shell follows the same rule. It can inspect and operate the platform only
because its manifest and bootstrap channel explicitly allow those actions.

## Supervision

All current platform services are long-running services. The manager:

- waits for registration before marking a service ready
- monitors task exit status
- restarts a service within its manifest restart budget
- treats exhausted restart budgets as fatal to the current root graph

Transient tools are separate from long-running services. The manager launches
them on shell request, binds any requested session handles, and returns only a
task handle back to the shell for observation.

## Deferred

This platform layer still does not implement:

- writable or user-owned storage
- directory capabilities for general applications
- network-backed package repositories or signed update feeds
- dynamic service installation
- richer routing, DHCP, DNS, TCP/UDP socket services, audio, or compatibility
  services
- richer terminal features and login/session policy
- signed repositories, writable install roots, and package-feed transport
- input-device hosts, shared-memory presentation buffers, and richer desktop
  shell policy beyond the current launcher/status/app surface model
