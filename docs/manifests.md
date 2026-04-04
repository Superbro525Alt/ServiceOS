# Service Manifests

## Current deployment model

Service manifests now live in the boot store under persisted bundle paths such
as:

- `services/console-service/manifest.svc`
- `services/config-service/manifest.svc`
- `services/log-service/manifest.svc`
- `services/status-service/manifest.svc`
- `services/shell-service/manifest.svc`
- `services/package-service/manifest.svc`

Package manifests now live beside those service manifests under paths such as:

- `packages/announce-service/1.0.0/package.pkg`
- `packages/announce-service/1.1.0/package.pkg`

The root manager loads `services/index.txt` from `storage-service`, then opens
each listed manifest through the same storage contract.

## Schema

Current fields:

- `service`: stable service identity used by the manager and registry
- `image`: executable image identifier stored in the boot store
- `startup`: startup mode, currently `eager` or `on-demand`
- `availability`: whether boot should treat the service as `required` or
  `optional`
- `ready_timeout`: manager-side ready deadline in ticks
- `restart`: restart policy, currently `on-failure:<count>[:<backoff-ticks>]`
- `depends`: comma-separated dependency list
- `grant`: one startup service-capability grant per line
- `lookup`: one lookup permission per line
- `resource`: one persisted resource path per line

Example:

```text
service=status-service
image=status-service
startup=eager
availability=required
ready_timeout=250
restart=on-failure:2:8
depends=log-service,config-service
grant=log-service:send
lookup=config-service:send
lookup=console-service:send
resource=services/status-service/resources/banner.txt
```

The current shell manifest uses the same schema to declare its dependencies and
lookup permissions without needing any shell-specific manifest escape hatch.

Package-delivered services now use the same manifest shape. The difference is
policy:

- always-on foundational services stay `startup=eager`
- repository-installed background services can now be `startup=on-demand`
- optional support services can now be marked `availability=optional` so the
  manager can degrade honestly instead of pretending the graph is fully healthy

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

## Package manifest schema

Current fields:

- `package`: stable package identity
- `version`: package version string
- `compat`: runtime/storage compatibility marker
- `service`: service identity provided by the package
- `service_manifest`: path to the service manifest to activate
- `activation`: current activation mode, currently `manual`
- `depends`: package-level dependency references, currently service identities
- `content`: one repository content path per line
- `integrity`: package metadata digest, currently `fnv64:0x...`

Example:

```text
package=announce-service
version=1.1.0
compat=serviceos.bootstore.v1
service=announce-service
service_manifest=packages/announce-service/1.1.0/service/manifest.svc
activation=manual
depends=log-service
content=packages/announce-service/1.1.0/service/manifest.svc
content=packages/announce-service/1.1.0/resources/message.txt
integrity=fnv64:0xd2b5f5606fc641cc
```

The package manifest does not replace the service manifest. It wraps one or
more versioned service bundles with lifecycle metadata that belongs to the
package/update layer rather than the root service manager.

## Restart policy

The current implementation now uses:

- `OnFailure { max_restarts, backoff_ticks }`

The manager combines that with:

- manifest `ready_timeout`
- required vs optional availability
- degraded-service state once restart limits are exceeded
- on-demand startup for package-installed services that should not be eager
  residents of the base graph

## Boot-store bundle layout

The boot store is a small read-only archive staged onto the EFI system
partition. It currently contains:

- executable flat images under `services/<name>/program.img`
- manifests under `services/<name>/manifest.svc`
- shared config blobs such as `config/system.cfg`
- service-specific resources such as
  `services/status-service/resources/banner.txt`
- repository-backed package bundles such as
  `packages/announce-service/<version>/...`

This is intentionally small and boot-focused. It is not yet a general package
format or mutable filesystem install root, but it is now enough to stage
versioned packages and activate them through the package service.

## Transfer-right model

The userspace message ABI carries explicit per-handle transfer-right fields.
That matters because the manager often needs to retain a stronger local handle
while delivering a weaker handle to the receiver.

This keeps capability distribution honest:

- manager registry handles can remain redistributable
- child-service handles can be send-only or otherwise reduced
- reply endpoints can be transferred with only the rights needed to answer
- transient tool-launch handles can be returned to the shell without granting
  broader manager authority

## Roadmap note

Open manifest and activation follow-on work is tracked centrally in
[docs/roadmap.md](roadmap.md). This page intentionally stays focused on the
current manifest format and implemented behavior.
