# Service Manifest Schema

## Current format

Service manifests are currently compiled into the root manager as static Rust
data. That is the right default for now because there is no filesystem or
package service yet.

The schema is still explicit and future-ready.

## Fields

- `id`: stable logical service identity used by the manager and registry
- `name`: human-readable service name for logs and diagnostics
- `image`: built-in executable image identifier
- `dependencies`: services that must be started first
- `mode`: lifecycle expectation for the service
- `grants`: capability grants that should be delivered at startup
- `restart`: supervision policy

## Modes

- `LongRunning`: the service must register a public endpoint and remain present
  as part of the active platform
- `OneShot`: the service is supervised until it completes successfully and may
  be restarted on failure

The mode distinction is important. Not every service should be treated as a
daemon.

## Capability grants

Each startup grant is explicit:

- `source`: which already-started service the capability originates from
- `rights`: which rights subset the child should receive

The manager duplicates a source handle with the requested reduced rights, then
transfers that duplicate to the child over the bootstrap control channel.

This keeps capability distribution declarative and reviewable in the manifest
instead of hiding it in service-specific startup code.

## Restart policy

- `Never`: the manager records failure but does not relaunch automatically
- `OnFailure { max_restarts }`: the manager retries after non-zero exit until
  the attempt budget is exhausted

## Future evolution

The format is intentionally small, but it is ready to grow with:

- filesystem-backed manifest sources
- signature or trust metadata
- richer health-check definitions
- on-demand activation policy
- per-service resource envelopes
- explicit bootstrap capability objects instead of the temporary root-role check
