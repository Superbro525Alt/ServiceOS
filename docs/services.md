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
  -> audio-service
       depends on log-service
       consumes one startup-granted audio-endpoint capability
  -> clipboard-service
  -> graphics-service
       depends on log-service
       consumes one startup-granted display-output capability
  -> session-service
       depends on graphics-service, log-service
  -> terminal-service
       depends on log-service, config-service, storage-service,
       status-service, package-service, network-service, graphics-service,
       session-service, desktop-shell-service
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
  -> runtime-service package
       activated on operator request
  -> developer-service package
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
  -> terminal-app
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
- `audio-service`
  - one audio-endpoint capability from the root bootstrap path
  - send-only handle to `log-service`
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
  - `audio-service` with send-only rights
- `package-service`
  - `storage-service` with send-only rights
- `developer-service`
  - `cross-builder-tool` launch authority through the root-manager bootstrap
    channel
- `session-service`
  - `graphics-service` with send-only rights
- `desktop-shell-service`
  - `graphics-service` with send-only rights
  - `session-service` with send-only rights
  - `network-service` with send-only rights
  - `status-service` with send-only rights
- `terminal-service`
  - `log-service` with send-only rights
  - `config-service` with send-only rights
  - `storage-service` with send-only rights
  - `status-service` with send-only rights
  - `package-service` with send-only rights
  - `network-service` with send-only rights
  - `graphics-service` with send-only rights
  - `session-service` with send-only rights
  - `desktop-shell-service` with send-only rights

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
- acquires dynamic IPv4 configuration when enabled, with a static fallback path
  from `config-service`
- loads static host mappings from a storage-backed resource blob
- exposes interface status, route reporting, DNS-backed host resolution, ICMP
  probes, and outbound TCP stream sessions through a stable service contract
- keeps the public contract generic so later VirtIO, additional virtual, and
  real-NIC backends can sit behind the same service boundary

### `audio-service`

- owns audio endpoint and playback stream policy in userspace
- consumes the explicit kernel audio-endpoint capability
- exposes endpoint status and playback-stream control through a stable service
- associates playback streams with session ids without collapsing session or
  desktop policy into the backend
- keeps the current QEMU PC-speaker backend behind a backend-neutral boundary
  so later hardware backends can fit the same service contract

### `clipboard-service`

- owns a small shared text clipboard buffer in userspace
- exposes explicit read/write clipboard operations through a service contract
- gives desktop apps a shared clipboard path without pushing clipboard policy
  into `desktop-shell-service` or per-app local state

### `graphics-service`

- owns display-output state in userspace
- consumes the explicit kernel display-output capability
- creates and tracks surfaces through per-surface handles
- composes the current surface set into the active output
- keeps compositor policy in userspace while leaving the kernel with only
  display-output mechanism

### `session-service`

- owns graphical session identity and focus policy
- owns the physical input ingress path for the active graphical session
- consumes the explicit bootstrap input-source capability
- looks up `graphics-service` rather than owning display hardware itself
- forwards physical pointer and keyboard events into the desktop interaction
  contract without absorbing product-layer window policy
- provides the initial basis for later login, desktop shell, and multi-session
  work

### `desktop-shell-service`

- is the first graphical product shell built on top of the platform services
- owns desktop chrome, launcher state, retained window state, and app focus
  tracking
- creates shell-owned surfaces through `graphics-service`
- retains the authoritative surface handle for each app window
- creates one app-control channel per launched app for focus, resize, close,
  pointer, key, and text delivery
- owns move, resize, minimize, maximize, restore, and close policy for desktop
  windows
- accepts desktop interaction requests for hit testing, pointer actions, and
  keyboard routing
- asks the root manager to launch graphical apps instead of spawning tasks
  directly

### `terminal-service`

- owns PTY-like terminal sessions for graphical terminal hosting
- reuses the shared shell command/runtime library instead of introducing a
  second shell implementation

### `developer-service`

- owns developer toolchain, workspace, build-job, and artifact policy in
  userspace
- is delivered as an optional package instead of being forced into the always-on
  base graph
- consumes explicit startup grants for `log-service`, `storage-service`, and
  its packaged catalog blob
- launches transient build workers through `root-manager` instead of spawning
  them directly
- exposes toolchain discovery, workspace discovery, build submission, job
  inspection, and artifact export through a stable service contract
- keeps target metadata backend-neutral so native, Linux, Windows, and future
  remote-target workflows fit the same boundary
- accepts terminal-session open, input, resize, status, and close requests
- keeps line editing, history, and prompt redraw logic out of the graphical UI
- logs terminal-session lifecycle through the normal logging path
- keeps desktop product policy out of `graphics-service` and `session-service`

### `settings-app`, `files-app`, and `monitor-app`

- are transient graphical applications rather than long-running platform
  services
- receive one surface handle, one app-control channel, and a small explicit
  service-handle set
- validate the current platform contracts for config, storage, status, network,
  and audio access
- stay replaceable and non-ambient: they do not inherit manager or compositor
  authority

### `shell-service`

- owns the first operator/developer command environment
- opens a console session through `console-service`
- inspects services through the manager control channel
- reads logs, config, and storage through explicit service lookups
- inspects networking and audio state through `network-service` and
  `audio-service`
- inspects and drives compatibility/runtime state through `runtime-service`
- inspects outputs, surfaces, and sessions through `graphics-service` and
  `session-service`
- launches transient tools through the manager rather than direct shell power

### `runtime-service`

- is the first compatibility/runtime platform service
- is package-delivered rather than always-on base graph infrastructure
- owns runtime environment creation, inspection, launch, and teardown
- maps guest-visible runtime paths onto explicit `storage-service` resources
- injects runtime variables from packaged runtime metadata
- launches runtime-hosted workloads through the existing root-manager tool path
- keeps compatibility/runtime policy in userspace instead of leaking it into
  native app semantics

### `posix-host-tool`

- is the first runtime-hosted transient workload image
- proves runtime launch, output relay, and mapped-resource access without
  claiming Linux ABI compatibility
- runs under `runtime-service` control rather than direct shell privilege

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

## Roadmap note

Open platform-service follow-on work is tracked centrally in
[docs/roadmap.md](roadmap.md). This page intentionally stays focused on the
current service boundaries and implemented contracts.
