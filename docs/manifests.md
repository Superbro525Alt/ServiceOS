# Service Manifests

## Current deployment model

Service manifests now live in the boot store under persisted bundle paths such
as:

- `services/console-service/manifest.svc`
- `services/config-service/manifest.svc`
- `services/log-service/manifest.svc`
- `services/status-service/manifest.svc`

The root manager loads `services/index.txt` from `storage-service`, then opens
each listed manifest through the same storage contract.

## Schema

Current fields:

- `service`: stable service identity used by the manager and registry
- `image`: executable image identifier stored in the boot store
- `startup`: current startup mode, currently `eager`
- `restart`: restart policy, currently `on-failure:<count>`
- `depends`: comma-separated dependency list
- `grant`: one startup service-capability grant per line
- `lookup`: one lookup permission per line
- `resource`: one persisted resource path per line

Example:

```text
service=status-service
image=status-service
startup=eager
restart=on-failure:2
depends=log-service,config-service
grant=log-service:send
lookup=config-service:send
lookup=console-service:send
resource=services/status-service/resources/banner.txt
```

## Startup grants and resources

Each grant is explicit:

- `grant=<service>:<rights>`

The manager duplicates the registered service handle locally, then transfers
only the declared rights to the child.

Each resource is also explicit:

- `resource=<boot-store path>`

The manager opens that path through `storage-service`, receives a blob
capability, and transfers only that blob capability to the child. Services do
not get ambient storage root authority just because they need one file.

## Lookup permissions

- `lookup=<service>:<rights>`

This is the current discovery policy. A service cannot look up arbitrary peers
just because it knows their name.

## Restart policy

The current implementation uses:

- `OnFailure { max_restarts }`

That is enough for the current always-on platform graph.

## Boot-store bundle layout

The boot store is a small read-only archive staged onto the EFI system
partition. It currently contains:

- executable flat images under `services/<name>/program.img`
- manifests under `services/<name>/manifest.svc`
- shared config blobs such as `config/system.cfg`
- service-specific resources such as
  `services/status-service/resources/banner.txt`

This is intentionally small and boot-focused. It is not yet a general package
format or mutable filesystem.

## Transfer-right model

The userspace message ABI carries explicit per-handle transfer-right fields.
That matters because the manager often needs to retain a stronger local handle
while delivering a weaker handle to the receiver.

This keeps capability distribution honest:

- manager registry handles can remain redistributable
- child-service handles can be send-only or otherwise reduced
- reply endpoints can be transferred with only the rights needed to answer

## Future evolution

The format is intentionally small, but it is ready to grow with:

- richer boot-store indexes and signatures
- trust metadata and signature policy
- richer health-check definitions
- on-demand activation policy
- directory capabilities and writable storage objects
