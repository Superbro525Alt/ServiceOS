# Status Service Contract

## Purpose

`status-service` is the first dependent long-running platform service.

It exists now to prove that a service can:

- depend on multiple foundational services
- discover peers through the manager
- read shared configuration
- produce structured logs
- expose its own small public contract

## Dependencies

- `log-service`
- `config-service`
- `console-service`

## Startup and discovery

At startup it receives a send-only logging handle from the root manager. It
then looks up `config-service` and `console-service` through the manager under
the manifest lookup policy.

## Public contract

Request:

- tag: `StatusTag::SnapshotRequest`
- handles:
  - `0`: reply endpoint

Reply:

- tag: `StatusTag::SnapshotReply`
- words:
  - `0`: heartbeat count
  - `1`: last emitted tick

## Current behavior

- reads heartbeat period and console mirror period from `config-service`
- emits structured heartbeat records to `log-service`
- mirrors every Nth heartbeat to `console-service`

## Deferred

- richer health reporting
- subscription-based monitoring
- integration with future shell/session status views
