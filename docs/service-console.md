# Console Service Contract

## Purpose

`console-service` is the current console-adjacent system I/O service.

It exists now to:

- own the direct route to the kernel debug output sink
- render structured service/platform events into readable lines
- establish a durable boundary between system output and log collection

## Public contract

Request:

- tag: `ConsoleTag::WriteRecord`
- words:
  - `0`: source `ServiceId`
  - `1`: `LogSeverity`
  - `2`: `LogDomain`
  - `3`: `LogEvent`
  - `4`: detail field 0
  - `5`: detail field 1
  - `6`: sequence number

There is no reply path today. Console writes are one-way best-effort output.

## Current users

- `log-service` forwards filtered structured logs here
- `status-service` mirrors periodic status summaries here

## Deferred

- terminal sessions
- input handling
- richer text rendering
- ownership transfer between shells or sessions
