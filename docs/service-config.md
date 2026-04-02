# Configuration Service Contract

## Purpose

`config-service` establishes the shared-configuration pattern for the platform.

It exists now to provide:

- capability-gated access to system settings
- a typed request/reply protocol
- a stable contract backed by default boot-store config plus persistent
  namespaced overrides

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

Write request:

- tag: `ConfigTag::WriteRequest`
- words:
  - `0`: `ConfigKey`
  - `1`: typed value payload
- handles:
  - `0`: reply endpoint with send rights for the response path

Write reply:

- tag: `ConfigTag::WriteReply`
- words:
  - `0`: requested `ConfigKey`
  - `1`: `ConfigStatus`

## Current keys

- `LogMinimumSeverity`
- `StatusHeartbeatTicks`
- `StatusConsoleMirror`
- `StatusHeartbeatLogPeriod`

## Current storage model

The current implementation reads `config/system.cfg` through a startup-granted
blob capability opened by `storage-service`, then overlays persistent
namespaced writes from `state/config/<namespace>/settings.cfg`.

Current namespaces are:

- `log`
- `status`
- `network`

Each override file is versioned and service-owned. `config-service` persists
updates through the real writable-storage path rather than writing directly to
ambient paths.

Validation and update policy stay inside `config-service`:

- known keys only
- type-specific range checks
- namespace-specific persistence paths
- versioned serialized override files for future migrations

The current default config keeps the operator shell quiet:

- `status.console_mirror=0` disables direct console heartbeat mirroring
- `status.heartbeat_log_period=0` disables recurring structured heartbeat logs

Set `status.heartbeat_log_period=1` when you want every heartbeat recorded for
debugging.

## Current operator path

The shell now uses the real config-service write path for:

- `config get <key>`
- `config set <key> <value>`

That means configuration updates are:

- validated by `config-service`
- persisted through `storage-service`
- reloaded across boot through namespaced override trees

## Roadmap note

Open configuration-service follow-on work is tracked centrally in
[docs/roadmap.md](roadmap.md). This page intentionally stays focused on the
current config-service contract and implemented behavior.
