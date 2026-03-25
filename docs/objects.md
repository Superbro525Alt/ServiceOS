# Kernel Objects, Capabilities, and IPC

## Scope

The kernel-side object model makes explicit authority and communication the
default system shape. It is the substrate that the current userspace service
manager and service graph build on top of.

## Object taxonomy

The generic object registry currently supports:

- task objects
- thread objects
- channel endpoint objects
- timer objects
- event objects
- memory objects

Each live object receives an `ObjectId` and an `ObjectKind`. The registry keeps
weak references so it can observe the live set without becoming the owner of
every object forever.

The task object is currently the process-equivalent abstraction. It owns:

- the address-space binding
- the capability space
- the set of member threads

## Capability model

Authority is carried by handles, not ambient global access.

Each task owns a `CapabilitySpace`. A handle entry contains:

- the target object reference
- a rights mask
- an optional badge value

Rights are intentionally small and mechanical:

- `READ`
- `WRITE`
- `MAP`
- `SIGNAL`
- `WAIT`
- `SEND`
- `RECEIVE`
- `DUPLICATE`
- `TRANSFER`
- `MANAGE`

Important rules:

- a handle may only be duplicated if it carries `DUPLICATE`
- a handle may only be transferred if it carries `TRANSFER`
- duplicated or transferred rights must be a subset of the source rights
- `close` removes the handle entry immediately
- object lifetime ends when no strong references remain

## IPC model

Channels are the first IPC primitive because they compose well into a
service-oriented system.

Current semantics:

- a channel pair is two endpoint objects linked to each other
- `send` requires `SEND` on the local endpoint handle
- `receive` requires `RECEIVE` on the local endpoint handle
- a message contains a word payload plus transferred capabilities
- a sender may attach a rights-reduced handle transfer to a message
- the receiver gets a fresh handle in its own capability space

The IPC layer intentionally stays minimal:

- no in-kernel RPC layer
- no broker or namespace policy
- no shared-memory protocol beyond a small future-facing hint field

## Lifetime and cleanup

The important invariant is that the registry indexes objects but does not own
them permanently.

In practice this means:

- handles keep objects alive because capability entries hold strong references
- temporary kernel variables also keep objects alive while they are in scope
- once handles are closed and temporary strong references are dropped, the
  registry can forget the object on the next garbage-collection pass

The kernel tests and runtime bootstrap exercise exactly this rule by
transferring object handles over channels, closing them, and verifying that the
registry contracts back to the remaining live roots.

## Still deferred

- userspace-visible memory-object mapping
- shared-memory IPC policy
- richer object inspection and wait primitives
