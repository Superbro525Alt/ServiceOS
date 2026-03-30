# Status Service Contract

## Purpose

`status-service` is the first dependent long-running platform service.

It exists now to prove that a service can:

- depend on multiple foundational services
- discover peers through the manager
- read shared configuration
- consume a startup-granted persisted resource
- produce structured logs
- expose its own small public contract

## Dependencies

- `log-service`
- `config-service`
- `console-service`

## Startup and discovery

At startup it receives a send-only logging handle from the root manager. It
also receives a blob capability for its banner resource, then looks up
`config-service` and `console-service` through the manager under the manifest
lookup policy.

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
- reads a separate heartbeat-log period from `config-service`
- reads a persisted banner resource from the boot store
- emits structured heartbeat records to `log-service`
- can suppress heartbeat logs entirely when `status.heartbeat_log_period=0`
- mirrors every Nth heartbeat to `console-service`

## Roadmap note

Open status and health follow-on work is tracked centrally in
[docs/roadmap.md](roadmap.md). This page intentionally stays focused on the
current status-service boundary and implemented behavior.
