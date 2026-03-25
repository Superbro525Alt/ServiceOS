# Console Service Contract

## Purpose

`console-service` is the current console-adjacent system I/O service.

It owns:

- the userspace route to the kernel debug log sink
- formatted lifecycle/log rendering for the wider service graph
- the first line-oriented operator session contract used by `shell-service`

## Public contracts

### Structured log rendering

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

This is one-way best-effort output. `log-service` uses it to publish the
filtered structured log stream.

### Session open

Request:

- tag: `ConsoleTag::SessionOpenRequest`
- handles:
  - `0`: reply endpoint

Reply:

- tag: `ConsoleTag::SessionOpenReply`
- handles:
  - `0`: session channel

### Session write

Request:

- tag: `ConsoleTag::SessionWriteText`
- words:
  - `0`: byte length
  - `1..`: packed UTF-8 bytes

This writes directly to the raw console stream without reformatting it as a
structured service log line.

### Session read

Request:

- tag: `ConsoleTag::SessionReadLineRequest`
- handles:
  - `0`: reply endpoint

Reply:

- tag: `ConsoleTag::SessionReadLineReply`
- words:
  - `0`: byte length
  - `1..`: packed UTF-8 line bytes

The current session contract is line-oriented and single-reader per session.

## Current users

- `log-service` forwards filtered structured logs here
- `shell-service` opens an operator session here
- transient tools such as `sysinfo-tool` write through a shell-granted session
  handle

## Deferred

- multiple concurrent operator sessions with routing policy
- terminal capabilities beyond simple line input and text output
- ownership transfer between shells or richer session managers
- terminal emulation and graphical console surfaces
