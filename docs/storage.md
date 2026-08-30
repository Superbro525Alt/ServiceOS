# Storage and Writable Capabilities

## Scope

The storage layer is still intentionally simple, but it is no longer
read-only-only or reboot-transient.

- one immutable boot store staged on the EFI system partition
- one userspace `storage-service` that owns access to that boot store
- one optional writable block device handed in as a bootstrap resource
- one persistent snapshot-backed writable store layered under the same storage
  contract
- read-only exact-path opens for persisted boot-store content
- writable directory and file capabilities for mutable namespaces
- blob capabilities for per-file reads and writes

This is enough to support real shell, app, and developer workflows without
pretending the system already has a full general-purpose filesystem.

## Persistent writable backing

On the active QEMU target, `storage-service` now mounts an optional writable
VirtIO block device and uses it as a durable backing store for mutable content.

The current persistence model is intentionally narrow and service-owned:

- `home/`
- `state/`
- `projects/`

Those namespaces are serialized into a versioned snapshot format owned by
`storage-service`. `tmp/` remains writable but intentionally ephemeral.

This is not yet a general-purpose mounted filesystem stack. It is a durable
block-backed writable layer under the existing storage contract so real file,
config, and developer-output workflows survive reboot without introducing
ambient write access.

The current snapshot format keeps:

- a small header with magic, version, generation, and layout offsets
- a record table for persisted mutable entries
- block-aligned file payload regions for mutable file contents

On boot, `storage-service` selects the newest valid snapshot generation and
reconstructs mutable entries into the same scoped directory/file capability
model used at runtime.

## Block-device foundation

The kernel now exposes block devices as explicit objects rather than burying
them in `storage-service` internals.

Current block-device support includes:

- block-device info
- block reads
- block writes
- platform registration of the QEMU VirtIO block backend
- bootstrap transfer of an optional block-device handle to `storage-service`

This keeps:

- platform device wiring in `platform/<platform>`
- kernel object and syscall semantics in `kernel/core`
- storage policy and snapshot format in `storage-service`

The current system now exposes a first real namespace surface on top of that
backing:

- explicit mount inventory through `MountListRequest`
- a composed namespace root through `DirectoryOpenRequest` on `""`
- relative path traversal through scoped directory capabilities via
  `DirectoryTraverseRequest`

It is still not a full general-purpose mount daemon or VFS. Mount mutation,
cross-backend composition, and richer namespace policy remain follow-on work.

## Boot-store path

The boot store is built on the host and staged as:

- `\serviceos\bootstore.bin`

At boot:

1. UEFI firmware loads the kernel EFI image.
2. the kernel reads `bootstore.bin` before `ExitBootServices`.
3. the kernel uses that immutable byte image as:
   - the executable source for `ServiceSpawn`
   - the boot-root storage capability passed into userspace

## Storage service contract

`storage-service` remains the storage policy service in userspace.

Its public contract is:

- `OpenRequest`
  - input: exact path string plus reply endpoint
  - output: read-only blob capability plus blob size
- `ReadRequest`
  - input: blob capability, offset, requested byte count, reply endpoint
  - output: a bounded byte chunk
- `DirectoryOpenRequest`
  - input: exact directory path plus writable intent
  - output: scoped directory capability
- `DirectoryReadRequest`
  - input: directory capability plus enumeration cursor
  - output: next child entry kind and path under that scoped directory
- `DirectoryCreateRequest`
  - input: directory capability plus child name and kind
  - output: status only
- `DirectoryRemoveRequest`
  - input: directory capability plus child name
  - output: status only
- `DirectoryOpenFileRequest`
  - input: directory capability plus child name, create flag, writable flag
  - output: scoped file/blob capability plus size
- `DirectoryTraverseRequest`
  - input: directory capability plus relative path, target kind, writable
    intent
  - output: scoped file/blob or directory capability below that namespace root
- `WriteRequest`
  - input: writable blob capability, offset, total length, byte payload
  - output: written length and updated file length
- `MountListRequest`
  - input: enumeration cursor on the storage root handle
  - output: mount path, backend kind, writable bit, persistent bit

That protocol is now broad enough for practical app and tool workflows:

- create/open/read/write/truncate/delete files
- create/open/list/remove directories
- enumerate children through scoped directory handles
- traverse nested subpaths from an already-opened directory capability
- inspect the current composed namespace mount table
- save native build outputs into persistent storage
- reopen and execute stored user-supplied images through the manager-owned
  loader path

Exact-path opens are still service-mediated. Enumeration and mutation now go
through explicit directory or writable-file capabilities instead of an ambient
writable root.

## Capability model

Storage access is capability-oriented:

- root manager gets the storage root endpoint
- config and status services get only the blob handles they require
- ordinary services do not get ambient path traversal authority
- writable access is scoped to explicit directory/file capabilities
- writable handles do not imply broader traversal or mutation rights elsewhere

The current mutable policy is intentionally narrow:

- mutable paths are limited to service-owned namespaces such as:
  - `home/`
- `tmp/`
- `state/`
- `projects/`
- boot-store paths remain immutable
- deletion is limited to mutable files and empty mutable directories

This keeps write authority useful without making the whole storage graph
ambiently mutable.

The current policy split is:

- boot-store content: immutable system and package-staged content
- `home/`: user-home style writable content
- `projects/`: developer and project outputs
- `state/`: service-owned persistent state such as config overrides
- `tmp/`: mutable but intentionally non-persistent scratch space

## What is persisted now

The current persisted boot store contains:

- service manifests
- service executable images
- system config data
- one service resource blob used by `status-service`

Executable images are now also openable through the normal storage path. That
lets the root manager launch stored images through a richer manager-owned
loader flow instead of requiring only built-in image ids.

## Current workflows

The live shell now uses the real writable-storage path for:

- `store mounts`
- `store mkdir <path>`
- `store write <path> <text>`
- `store rm <path>`
- `cat <path>`
- `run image <path>`

That makes simple project output, notes, state files, and config writing
practical inside the current system.

Developer workflows now also use the same storage path for persistent build
outputs:

- `dev build 0 native`
- `dev save 0 projects/hello-cross.img`
- `run image projects/hello-cross.img`

That proves stored user-supplied images are no longer trapped in a transient
bootstrap-only path.

`files-app` now also enumerates directories through an opened directory
capability rather than through root-handle path walking. That keeps browsing,
create, open-for-write, and removal flows aligned around the same scoped
authority model.

From a directory view, printable typing now switches `files-app` into bounded
name search using the storage index. The app sends the current directory path
as the subtree scope, keeps at most its fixed 64 visible hits, and preserves the
service's exact/prefix/substring ranking. Backspace edits the query, while an
empty query or Escape reloads the ordinary directory listing. Enter navigates
directory hits or sends file hits through the same open-with and recent-files
path as normal browsing. This is filename discovery only; the app does not
expose the storage grep primitive or an editor.

The shell now also opens the namespace root once, then traverses or mutates
through scoped directory capabilities instead of relying on repeated root-path
opens for ordinary file operations.

## Roadmap note

Open storage and writable-capability follow-on work is tracked centrally in
[docs/roadmap.md](roadmap.md). This page intentionally stays focused on the
current storage architecture and implemented behavior.
