# Package and Update Foundation

## Role

`package-service` is the first software lifecycle manager in userspace.

It owns:

- repository package discovery
- package metadata inspection
- install, update, remove, and rollback requests
- package activation policy for package-provided services
- coordination with the root manager for service lifecycle transitions
- package lifecycle logging

It does not own storage policy, process spawning, or service supervision.

- `storage-service` still owns persisted object access
- the root manager still owns service startup, readiness, restart, and dynamic
  activation/deactivation execution
- the shell is only an operator client of `package-service`

## Package model

The current package repository is staged into the boot store under
`packages/...`.

Each package version currently wraps:

- one package manifest (`package.pkg`)
- one service manifest to activate
- any versioned static resources referenced by that service manifest, including
  runtime profiles and mounted runtime-root content

Current package-backed components include:

- `announce-service`, with repository versions `1.0.0` and `1.1.0`
- `runtime-service`, which delivers the first compatibility/runtime service,
  runtime profile metadata, and a small mounted runtime root tree
- `developer-service`, which delivers the first developer-toolchain catalog,
  workspace descriptors, sample project content, and SDK metadata placeholders

## Authority model

Package authority is explicit.

- the root manager starts `package-service` as a normal service
- `package-service` gets only the capabilities it needs:
  - a send-only startup grant to `log-service`
  - lookup access to `storage-service`
  - its normal bootstrap/control channel back to the root manager
- `shell-service` gets lookup access to `package-service`
- ordinary services do not get package authority by default

Installing or updating software is therefore a consequence of holding the
`package-service` capability, not ambient global power.

## Activation and update flow

Current install flow:

1. the shell sends a package request to `package-service`
2. `package-service` resolves the package version from repository metadata
3. `package-service` opens the referenced bundle content through
   `storage-service`
4. `package-service` asks the root manager to activate the referenced service
   manifest
5. the root manager loads the service manifest, starts the service, waits for
   registration, and then reports success or failure

Current update flow:

1. `package-service` selects a newer repository version
2. `package-service` asks the root manager to replace the currently active
   dynamic service slot for the target `ServiceId`
3. `package-service` updates its active/rollback state only after successful
   activation

Current rollback flow:

1. `package-service` remembers the prior active version
2. a remove or failed update can be followed by `pkg rollback <name>`
3. the prior service manifest is reactivated through the root manager

## Recovery model

The current recovery model is intentionally small but real.

- activation is a state transition with success/failure reporting
- remove does not erase rollback state
- rollback is explicit and observable
- package lifecycle events are logged through the normal structured log path

Current recovery is service-scoped, not whole-system transactional image
recovery.

## Integrity metadata

Packages now carry `integrity=fnv64:0x...` metadata.

At this stage the repository is still the trusted boot-store image staged by
the build system, so the digest is treated as repository metadata and content
shape validation rather than a final trust root. Full hard enforcement and
signature policy are deferred until the system has writable repositories and
signed feeds. That staged-source assumption is intentional and temporary; it is
not the final package trust model.

## Example workflow

The current shell can exercise the full package foundation:

1. `pkg list`
2. `pkg install announce 1.0.0`
3. `pkg update announce`
4. `pkg history announce`
5. `pkg remove announce`
6. `pkg rollback announce`

That workflow proves:

- repository package discovery
- explicit package authority
- version selection
- root-manager activation of package-provided services
- remove and rollback behavior
- visible package lifecycle logs

The runtime workflow builds on the same package path:

1. `pkg install runtime`
2. `runtime-service` is activated through the root manager
3. the runtime package's profile and root content become available as explicit
   resources to that service

The developer workflow uses the same lifecycle:

1. `pkg install developer`
2. `developer-service` is activated through the root manager
3. the package's toolchain catalog, workspace descriptors, source payloads, and
   SDK metadata become explicit resources for that service
4. build workers are launched later by `developer-service`, not by
   `package-service`

## Roadmap note

Open package and update follow-on work is tracked centrally in
[docs/roadmap.md](roadmap.md). This page intentionally stays focused on the
current package/update architecture and implemented behavior.
