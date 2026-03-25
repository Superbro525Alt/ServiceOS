# Foundational Userspace Services

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
  -> status-service
       depends on log-service, config-service
  -> shell-service
       depends on console-service, log-service, config-service,
       storage-service, status-service
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

### `shell-service`

- owns the first operator/developer command environment
- opens a console session through `console-service`
- inspects services through the manager control channel
- reads logs, config, and storage through explicit service lookups
- launches transient tools through the manager rather than direct shell power

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
- package or update policy
- dynamic service installation
- richer terminal features, login/session policy, networking, graphics, audio,
  or compatibility services
