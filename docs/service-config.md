# Configuration Service Contract

## Purpose

`config-service` establishes the shared-configuration pattern for the platform.

It exists now to provide:

- capability-gated access to system settings
- a typed request/reply protocol
- a stable contract future storage-backed configuration can preserve

## Public contract

Request:

- tag: `ConfigTag::ReadRequest`
- words:
  - `0`: `ConfigKey`
- handles:
  - `0`: reply endpoint with send rights for the response path

Reply:

- tag: `ConfigTag::ReadReply`
- words:
  - `0`: requested `ConfigKey`
  - `1`: `ConfigValueKind`
  - `2`: typed value payload

## Current keys

- `LogMinimumSeverity`
- `StatusHeartbeatTicks`
- `StatusConsoleMirror`

## Current storage model

The current implementation is a static in-memory table compiled into the
service. That is intentional for now.

## Deferred

- persistent configuration storage
- namespaced service configuration trees
- write/update policy
- validation and schema migration logic
