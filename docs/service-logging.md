# Logging Service Contract

## Purpose

`log-service` is the durable userspace logging destination for the current
platform.

It exists now to establish:

- structured inter-service logging
- severity-based filtering
- a stable sink for service-manager lifecycle events
- a clean split between log collection and output routing

## Dependencies

- `console-service`
- `config-service`

## Startup grants

`log-service` receives:

- a send-only handle to `console-service`
- a send-only handle to `config-service`

## Public contract

Public endpoint protocol:

- tag: `LogTag::Record`
- words:
  - `0`: source `ServiceId`
  - `1`: `LogSeverity`
  - `2`: `LogDomain`
  - `3`: `LogEvent`
  - `4`: detail field 0
  - `5`: detail field 1

The service filters by configured minimum severity, assigns a sequence number,
and forwards the resulting record to `console-service`.

## Current behavior

- minimum severity is read from `config-service`
- records at or above that severity are forwarded
- service-manager lifecycle events are emitted into the same log stream

## Deferred

- persistent log storage
- subscription/multiplexing
- richer structured payload schemas
- log ingestion from kernel traps beyond the current debug route
