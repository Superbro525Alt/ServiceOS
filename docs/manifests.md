# Service Manifests

## Current format

Service manifests are currently compiled into the root manager as static Rust
data. That is the right default for now because there is no filesystem or
package service yet.

The schema is still explicit and future-ready.

## Fields

- `id`: stable logical service identity used by the manager and registry
- `name`: human-readable service name for logs and diagnostics
- `image`: built-in executable image identifier
- `dependencies`: services that must already be ready before startup
- `grants`: startup capability grants that should be transferred immediately
- `lookups`: services this service may discover later through the manager
- `restart`: supervision policy

## Startup grants

Each startup grant is explicit:

- `source`: which already-ready service the capability originates from
- `rights`: the rights subset the child should receive

The manager duplicates a stronger local handle, then transfers only the declared
rights to the child over the bootstrap control channel.

## Lookup permissions

Each lookup permission is also explicit:

- `target`: which registered service may be looked up
- `rights`: which rights subset the caller may receive

This is the current discovery policy. A service cannot look up arbitrary peers
just because it knows their name.

## Restart policy

The current implementation uses:

- `OnFailure { max_restarts }`: restart the service until its retry budget is
  exhausted

That is enough for the current always-on foundational graph.

## Transfer-right model

The userspace message ABI now carries explicit per-handle transfer-right fields.
That matters because the manager often needs to retain a stronger local handle
while delivering a weaker handle to the receiver.

This keeps capability distribution honest:

- manager registry handles can remain redistributable
- child-service handles can be send-only or otherwise reduced
- reply endpoints can be transferred with only the rights needed to answer

## Future evolution

The format is intentionally small, but it is ready to grow with:

- filesystem-backed manifest sources
- trust metadata and signature policy
- richer health-check definitions
- on-demand activation policy
- per-service resource envelopes
- explicit bootstrap capability objects instead of the temporary root-role gate
