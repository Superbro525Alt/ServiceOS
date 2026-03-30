# Logging Service Contract

## Purpose

`log-service` is the durable userspace logging destination for the current
platform.

It establishes:

- structured inter-service logging
- severity-based filtering
- a stable sink for service-manager lifecycle events
- a queryable in-memory history for the shell and future diagnostics tools
- a clean split between log collection and output routing

## Dependencies

- `console-service`
- `config-service`

## Startup grants

`log-service` receives:

- a send-only handle to `console-service`
- a send-only handle to `config-service`

## Public contracts

### Record ingest

- tag: `LogTag::Record`
- words:
  - `0`: source `ServiceId`
  - `1`: `LogSeverity`
  - `2`: `LogDomain`
  - `3`: `LogEvent`
  - `4`: detail field 0
  - `5`: detail field 1

The service filters by configured minimum severity, assigns a sequence number,
stores the record in a bounded ring, and forwards it to `console-service`.

### History info

- tag: `LogTag::QueryInfoRequest`
- handles:
  - `0`: reply endpoint

Reply:

- tag: `LogTag::QueryInfoReply`
- words:
  - `0`: oldest retained sequence
  - `1`: next sequence value

### Record lookup

- tag: `LogTag::QueryRecordRequest`
- words:
  - `0`: requested sequence
- handles:
  - `0`: reply endpoint

Reply:

- tag: `LogTag::QueryRecordReply`
- words:
  - `0`: `LogQueryStatus`
  - `1`: sequence
  - `2`: source `ServiceId`
  - `3`: `LogSeverity`
  - `4`: `LogDomain`
  - `5`: `LogEvent`
  - `6`: detail field 0
  - `7`: detail field 1

## Current behavior

- minimum severity is read from `config-service`
- records at or above that severity are retained and forwarded
- service-manager lifecycle events and shell events share the same stream
- the shell uses the query interface rather than bypassing the log service

## Roadmap note

Open follow-on logging work is tracked centrally in
[docs/roadmap.md](roadmap.md). This page intentionally stays focused on the
current logging design and implemented behavior.
