# Storage and Writable Capabilities

## Scope

The storage layer is still intentionally simple, but it is no longer
read-only-only.

- one immutable boot store staged on the EFI system partition
- one userspace `storage-service` that owns access to that boot store
- read-only exact-path opens for persisted boot-store content
- writable directory and file capabilities for mutable namespaces
- blob capabilities for per-file reads and writes

This is enough to support real shell, app, and developer workflows without
pretending the system already has a full general-purpose filesystem.

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
- `DirectoryCreateRequest`
  - input: directory capability plus child name and kind
  - output: status only
- `DirectoryRemoveRequest`
  - input: directory capability plus child name
  - output: status only
- `DirectoryOpenFileRequest`
  - input: directory capability plus child name, create flag, writable flag
  - output: scoped file/blob capability plus size
- `WriteRequest`
  - input: writable blob capability, offset, total length, byte payload
  - output: written length and updated file length

Exact-path opens are still service-mediated. Mutation now goes through explicit
directory or writable-file capabilities instead of an ambient writable root.

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

## What is persisted now

The current persisted boot store contains:

- service manifests
- service executable images
- system config data
- one service resource blob used by `status-service`

Executable images are now also openable through the normal storage path, which
lets the root manager launch stored images through a richer manager-owned loader
flow instead of requiring only built-in image ids.

## Current workflows

The live shell now uses the real writable-storage path for:

- `store mkdir <path>`
- `store write <path> <text>`
- `store rm <path>`
- `cat <path>`

That makes simple project output, notes, state files, and config writing
practical inside the current system.

## Deferred

Still intentionally deferred:

- persistent writable backing beyond the current in-memory mutable overlay
- broader user home/storage policy
- mount management
- integrity/signature policy
- package/update logic
- broader application-facing filesystem APIs
