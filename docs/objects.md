# Kernel Objects, Capabilities, and IPC

## Phase 3 scope

Phase 3 establishes the kernel-side composition model for the future
service-oriented OS. The goal is not to implement services yet. The goal is to
make explicit authority and communication the default kernel shape.

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

- the future address-space binding
- the capability space
- the set of member threads

## Capability model

Authority is carried by handles, not ambient global access.

Each task owns a `CapabilitySpace`. A handle entry contains:

- the target object reference
- a rights mask
- an optional badge value

Rights are intentionally small and mechanical in Phase 3:

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
- the duplicated or transferred rights must be a subset of the source rights
- `close` removes the handle entry immediately
- object lifetime ends when no strong references remain, after which the
  registry can drop the weak entry during garbage collection

## IPC model

Channels are the first IPC primitive because they compose well into a future
service graph.

Current semantics:

- a channel pair is two endpoint objects linked to each other
- `send` requires `SEND` on the local endpoint handle
- `receive` requires `RECEIVE` on the local endpoint handle
- a message contains a small word payload plus up to four transferred
  capabilities
- a sender may attach a rights-reduced handle transfer to a message
- the receiver gets a fresh handle in its own capability space

Phase 3 intentionally keeps IPC minimal:

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

The QEMU boot self-check exercises exactly this rule by transferring a memory
object handle over a channel, closing both sender and receiver handles, and
verifying that the registry contracts back to the bootstrap root task.

## Deferred to later phases

What still remains after Phase 4:

- userspace-visible handle syscalls
- memory-object mapping into user address spaces
- shared-memory IPC policy
- service discovery, naming, or launch policy
