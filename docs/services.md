# Foundational Services

## Why these services exist now

This stage is about turning the root bootstrap into a real platform layer
without jumping into storage, networking, GUI, or package policy.

The current foundational set is intentionally small:

- `console-service`
- `config-service`
- `log-service`
- `status-service`

Together they establish:

- stable service contracts
- explicit capability-based access between services
- a clean lookup model
- lifecycle management for always-on system services
- a pattern future platform services can follow

## Dependency graph

```text
root-manager
  -> console-service
  -> config-service
  -> log-service
       depends on console-service, config-service
  -> status-service
       depends on log-service, config-service, console-service
```

## Root manager responsibilities

The root manager now owns:

- manifest evaluation
- startup ordering
- startup capability grants
- service registration
- controlled lookup/discovery
- restart supervision for long-running services

The kernel still only provides process/thread creation, handle spaces, IPC,
timers, and scheduling mechanisms.

## Capability distribution model

The root manager does not start services with global power.

Instead it:

- spawns each child with a private bootstrap control channel
- duplicates only the specific startup handles declared in the manifest
- transfers only the declared rights for each handle
- retains stronger registry handles when a service registers itself
- mediates later lookups through per-service lookup policy

Current startup grants:

- `log-service` receives:
  - a send-only handle to `console-service`
  - a send-only handle to `config-service`
- `status-service` receives:
  - a send-only handle to `log-service`

Current lookup permissions:

- `status-service` may look up:
  - `config-service` with send-only rights
  - `console-service` with send-only rights

There is no ambient namespace where every service can connect to every other
service.

## Service roles

### `console-service`

- owns the immediate route to the kernel debug output sink
- renders structured records into readable text
- acts as the first console-adjacent system I/O service

### `config-service`

- serves small typed configuration values
- establishes the request/reply pattern for shared system configuration
- provides a stable capability-gated contract that later storage-backed config
  can preserve

### `log-service`

- is the durable destination for service log records
- filters by configured minimum severity
- tags records by source service and domain
- forwards readable output through `console-service`
- receives service-manager lifecycle events as normal structured records

### `status-service`

- is the first long-running dependent platform service
- discovers peer services through the manager rather than ambient access
- reads configuration, logs structured heartbeats, and optionally mirrors
  periodic status to the console
- exposes a small snapshot contract for future readers

## Registry and discovery

The registry is manager-mediated and identity-based.

- services register themselves under a stable `ServiceId`
- the manager stores the registered public endpoint handle
- callers must request a lookup through their control channel
- the manager checks the manifest lookup policy before granting access
- replies return a newly duplicated handle with the requested reduced rights

This keeps discovery explicit and compatible with future richer namespaces.

## Supervision

All current foundational services are long-running services. The manager:

- waits for registration before considering a service ready
- monitors task exit status
- restarts failed services within the manifest restart budget
- treats exhausted restart budgets as fatal for the current bootstrap graph

## Deferred

This platform layer still does not implement:

- filesystem-backed manifests
- dynamic service installation
- persistent configuration storage
- richer service health probes
- user fault delivery back into the manager
- networking, storage, graphics, audio, shell, or package services
