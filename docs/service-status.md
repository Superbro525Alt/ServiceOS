# Status Service Contract

## Purpose

`status-service` is the first dependent long-running platform service.

It exists now to prove that a service can:

- depend on multiple foundational services
- discover peers through the manager
- read shared configuration
- consume a startup-granted persisted resource
- produce structured logs
- expose a small but real structured status bus

## Dependencies

- `log-service`
- `config-service`
- `console-service`

## Startup and discovery

At startup it receives a send-only logging handle from the root manager. It
also receives a blob capability for its banner resource, then looks up
`config-service` and `console-service` through the manager under the manifest
lookup policy. It seeds its baseline per-service view from the root-manager
graph and then accepts later manager-published phase and health updates.

## Public contract

### Snapshot

Request:

- tag: `StatusTag::SnapshotRequest`
- handles:
  - `0`: reply endpoint

Reply:

- tag: `StatusTag::SnapshotReply`
- words:
  - `0`: heartbeat count
  - `1`: last emitted tick
  - `2`: tracked service count

### Manager-published service report

`root-manager` publishes manager-owned service state into `status-service` using
`StatusTag::ServiceReport`.

Words:

- `0`: subject `ServiceId`
- `1`: `ManagerServicePhase`
- `2`: `StatusHealth`
- `3`: detail kind
- `4`: detail field 0
- `5`: detail field 1
- `6`: updated tick

The current detail kinds are:

- `status_detail_kind::LIFECYCLE`
- `status_detail_kind::BLOCKED_DEPENDENCY`
- `status_detail_kind::RESTART_BACKOFF`
- `status_detail_kind::HEARTBEAT`

### Per-service query

Request:

- tag: `StatusTag::ServiceQueryRequest`
- words:
  - `0`: requested `ServiceId`
- handles:
  - `0`: reply endpoint

Reply:

- tag: `StatusTag::ServiceQueryReply`
- words:
  - `0`: `StatusResult`
  - `1`: `ServiceId`
  - `2`: `ManagerServicePhase`
  - `3`: `StatusHealth`
  - `4`: detail kind
  - `5`: detail field 0
  - `6`: detail field 1
  - `7`: updated tick

### Per-service list

Request:

- tag: `StatusTag::ServiceListRequest`
- words:
  - `0`: page cursor
- handles:
  - `0`: reply endpoint

Reply:

- tag: `StatusTag::ServiceListReply`
- words:
  - `0`: emitted entry count
  - `1`: next page cursor or `u64::MAX`
  - remaining words: repeated 7-word service status entries in the same layout
    as the per-service query payload

### Subscription

Request:

- tag: `StatusTag::SubscribeRequest`
- words:
  - `0`: optional `ServiceId` filter, or `root-manager` for all events
- handles:
  - `0`: subscription endpoint
  - `1`: reply endpoint

Reply:

- tag: `StatusTag::SubscribeReply`
- words:
  - `0`: `StatusResult`

Stream event:

- tag: `StatusTag::StreamEvent`
- words:
  - `0`: `ServiceId`
  - `1`: `ManagerServicePhase`
  - `2`: `StatusHealth`
  - `3`: detail kind
  - `4`: detail field 0
  - `5`: detail field 1
  - `6`: updated tick

## Current behavior

- reads heartbeat period and console mirror period from `config-service`
- reads a separate heartbeat-log period from `config-service`
- reads a persisted banner resource from the boot store
- emits structured heartbeat records to `log-service`
- can suppress heartbeat logs entirely when `status.heartbeat_log_period=0`
- mirrors every Nth heartbeat to `console-service`
- maintains a manager-backed per-service health table for foundational services
- supports query, list, and subscription-based monitoring from shell and later
  desktop/operator clients
- carries status-service heartbeat detail inside the same per-service table

## Roadmap note

Open status and health follow-on work is tracked centrally in
[docs/roadmap.md](roadmap.md). This page intentionally stays focused on the
current status-service boundary and implemented behavior.
