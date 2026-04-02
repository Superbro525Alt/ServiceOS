# Package Repositories, Trust, and Update Foundation

## Role

`package-service` is the software distribution authority in userspace.

It owns:

- repository registration and synchronization
- package catalog and metadata inspection
- trust/provenance reporting for packages and repositories
- install, update, remove, rollback, and package-policy decisions
- writable install roots, install journals, and package-state persistence
- package consistency validation, repair, and garbage collection
- coordination with the root manager for activation and deactivation

It does not own storage hardware, network transport, service supervision, or
desktop UX.

- `storage-service` still owns persistent file and directory access
- `network-service` still owns transport
- `root-manager` still owns service activation/deactivation execution
- `software-center-app` and `shell-service` are clients of `package-service`,
  not alternate package backends

## Repository model

The package model now has two repository classes:

- the built-in boot repository staged into `packages/...`
- writable registered remote repositories synced over HTTP into
  `state/packages/repos/...`

Repository state is persisted in `state/packages/repos.cfg` and includes:

- repository name
- repository URL
- trust mode
- channel
- ring
- enable/disable state
- pinned digest
- last synced digest
- last sync state

The built-in repository remains immutable and boot-trusted. Additional
repositories are operator-registered and live entirely behind the package
service contract.

## Feed format and trust model

Remote repositories currently expose a compact text feed:

```text
version=1
entry=<package>|<service>|<version>|<compat>|<manifest>|<category>|<summary>
```

This phase intentionally keeps the feed format small and replaceable. The trust
model is materially stronger than the old boot-only package index, but it is
not yet a full public-key package ecosystem.

Current trust modes:

- `boot`
  - only for the built-in repository
  - trusted because it ships in the boot image
- `unsigned`
  - remote metadata is accepted but reported as unverified
- `pinned-digest`
  - the feed must match the configured FNV64 digest
  - a mismatch blocks sync and is surfaced as verification failure

Current trust states surfaced to clients:

- `boot-trusted`
- `unverified`
- `digest-pinned`
- `verification-failed`

This is intentionally honest:

- there is real provenance and policy state
- there is real verification for pinned-digest repositories
- there is not yet cryptographic signature verification or trust-root rotation

## Install roots and persistent state

Package-managed writable state now lives under `state/packages/`.

Current package persistence layout:

- `state/packages/repos.cfg`
  - registered repositories and their trust/policy state
- `state/packages/repos/<repo>/feed.idx`
  - last synced repository feed cache
- `state/packages/installed.cfg`
  - installed/active/rollback version state plus channel/ring/pin policy
- `state/packages/journal.cfg`
  - interrupted operation journal
- `state/packages/install/<package>/<version>/`
  - writable materialized install root for remote package content

Remote packages are materialized into writable install roots before activation.
Their manifests are rewritten so activation happens from persisted storage
rather than the original repository URL.

## Package policy

Per-package policy is explicit and persisted.

Current policy controls:

- pinned version
- channel selection: `stable`, `beta`, `canary`
- ring selection: `production`, `preview`, `testing`

Version selection uses those policy settings when choosing install/update
targets from the available repository versions.

This gives the system a real policy surface without pretending it already has a
full staged-rollout or enterprise policy engine.

## Install, update, rollback, and recovery

Current install/update flow:

1. a client opens `package-service`
2. `package-service` resolves the target version from repository metadata and
   package policy
3. if the package version is remote, `package-service` fetches and materializes
   it into `state/packages/install/...`
4. the manifest and content integrity are validated
5. `package-service` asks the root manager to activate the package's service
   manifest
6. installed/active/rollback state is updated only after successful activation

Current rollback and recovery behavior:

- package operations are journaled in `state/packages/journal.cfg`
- failed activation attempts preserve the previous active version when possible
- `pkg rollback <name>` reactivates the stored rollback version
- `pkg verify` validates local package state against persisted manifests
- `pkg repair` clears interrupted journal state and repairs broken local state
- `pkg gc` removes obsolete materialized versions that are no longer active or
  rollback targets

This is real package-level recovery and cleanup. It is not yet a full
whole-system transactional image updater.

## Provenance and operator surfaces

The package contract now exposes:

- package catalog browsing
- repository listing and sync state
- package provenance, trust state, source path, and rollback provenance
- package policy inspection and mutation
- maintenance state for validate/repair/garbage-collect flows

Current shell/operator workflows include:

- `pkg catalog`
- `pkg repos`
- `pkg repo add <name> <url> [unsigned|pinned:<hex>] [stable|beta|canary] [production|preview|testing]`
- `pkg repo sync [all|index]`
- `pkg provenance <name>`
- `pkg policy <name>`
- `pkg pin <name> <version|none>`
- `pkg channel <name> <stable|beta|canary>`
- `pkg ring <name> <production|preview|testing>`
- `pkg verify`
- `pkg repair`
- `pkg gc`

## Software center relationship

`software-center-app` is a graphical client of the same package contract.

It currently:

- browses the package catalog
- shows package category and summary
- shows source/trust/channel/ring/rollback metadata for the selected package
- syncs repositories
- installs, updates, and removes the selected package

It does not bypass package policy or trust checks. It uses the same
`package-service` operations that the shell uses.

## Example workflows

Current terminal flow:

1. `pkg repos`
2. `pkg repo add demo http://host/packages/feed.idx unsigned`
3. `pkg repo sync all`
4. `pkg catalog`
5. `pkg provenance runtime`
6. `pkg install runtime`
7. `pkg policy runtime`
8. `pkg verify`

Current graphical flow:

1. `desktop launch software`
2. review the package catalog and provenance details
3. sync repositories
4. install, update, or remove the selected package through the same backend

## Roadmap note

Open package/distribution follow-on work is tracked centrally in
[docs/roadmap.md](roadmap.md). This page intentionally stays focused on the
current repository/trust/package architecture and implemented behavior.
