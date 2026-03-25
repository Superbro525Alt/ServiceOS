# Storage And Runtime Foundation

## Scope

The current storage layer is intentionally narrow:

- one immutable boot store staged on the EFI system partition
- one userspace `storage-service` that owns access to that boot store
- exact-path open from the root manager
- blob capabilities for per-file reads

This is enough to prove persisted manifests, config, resources, and executable
inputs without pretending the system already has a general-purpose filesystem.

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

`storage-service` is the first real storage policy component in userspace.

Its public contract is:

- `OpenRequest`
  - input: exact path string plus reply endpoint
  - output: blob capability plus blob size
- `ReadRequest`
  - input: blob capability, offset, requested byte count, reply endpoint
  - output: a bounded byte chunk

Only the root manager currently receives the storage root endpoint. Other
services receive only the blob capabilities the manager explicitly opens and
passes to them.

## Capability model

Storage access is capability-oriented:

- root manager gets the storage root endpoint
- config and status services get only the blob handles they require
- ordinary services do not get ambient path traversal authority
- future directory or writable capabilities can layer on top of the same model

## What is persisted now

The current boot store contains:

- service manifests
- service executable images
- system config data
- one service resource blob used by `status-service`

This is enough to prove the deployment model and service/resource contracts.

## Deferred

Still intentionally deferred:

- writable files or directories
- user home/storage policy
- mount management
- integrity/signature policy
- package/update logic
- broader application-facing filesystem APIs
