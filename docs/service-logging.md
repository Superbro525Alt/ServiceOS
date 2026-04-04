# Logging Service Contract

## Purpose

`log-service` is the durable userspace logging destination for the current
platform.

It establishes:

- structured inter-service logging
- severity-based filtering
- a stable sink for service-manager lifecycle events
- a bounded persisted history under writable storage
- query and live-subscription access for shell and future diagnostics tools
- kernel trap ingestion through the real log pipeline rather than serial-only
  output
- a clean split between log collection and output routing

## Dependencies

- `console-service`
- `config-service`
- `storage-service` lookup permission for persistent log backing

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
  - `6`: detail field 2

The service filters by configured minimum severity, assigns a sequence number,
stamps it with a monotonic tick, stores it in a bounded ring, persists that
ring snapshot into `state/log/records.bin`, and forwards it to
`console-service`.

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
  - `2`: monotonic tick
  - `3`: source `ServiceId`
  - `4`: `LogSeverity`
  - `5`: `LogDomain`
  - `6`: `LogEvent`
  - `7`: detail field 0
  - `8`: detail field 1
  - `9`: detail field 2

### Streaming subscription

- tag: `LogTag::SubscribeRequest`
- words:
  - `0`: minimum severity
  - `1`: source filter or `LOG_FILTER_ANY`
  - `2`: domain filter or `LOG_FILTER_ANY`
- handles:
  - `0`: subscription endpoint
  - `1`: reply endpoint

Reply:

- tag: `LogTag::SubscribeReply`
- words:
  - `0`: `LogStatus`

Stream event:

- tag: `LogTag::StreamRecord`
- words:
  - `0`: sequence
  - `1`: monotonic tick
  - `2`: source `ServiceId`
  - `3`: `LogSeverity`
  - `4`: `LogDomain`
  - `5`: `LogEvent`
  - `6`: detail field 0
  - `7`: detail field 1
  - `8`: detail field 2

### Kernel event ingestion

`log-service` drains the kernel event ring through the syscall substrate and
re-emits trap records as:

- `source = root-manager`
- `domain = kernel`
- `event = kernel-trap`

That keeps low-level failures in the same queryable and persisted stream as
userspace service lifecycle events.

## Current behavior

- minimum severity is read from `config-service`
- records at or above that severity are retained, persisted, and forwarded
- service-manager lifecycle events and shell events share the same stream
- the shell uses the query and stream interfaces rather than bypassing the log
  service
- kernel traps are no longer visible only through transient early-console text

## Roadmap note

Open follow-on logging work is tracked centrally in
[docs/roadmap.md](roadmap.md). This page intentionally stays focused on the
current logging design and implemented behavior.
