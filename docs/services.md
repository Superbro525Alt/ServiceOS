# Root Service Bootstrap

## Role of the root manager

The first userspace process is now a real root service manager rather than a
demo program. Its job is to turn kernel mechanisms into a userspace-owned
service graph.

The kernel still owns:

- process and thread creation
- address-space construction
- handles, rights, and IPC transport
- timer and scheduling mechanisms

The root manager owns:

- which services start
- startup order
- which service receives which capabilities
- which services are long-running versus one-shot
- restart and failure policy
- service registration and discovery policy

This keeps high-level system coordination in userspace where it can evolve.

## Kernel to root contract

The current contract is intentionally narrow:

- the kernel launches the built-in root-manager image as the first user task
- that task runs with the bootstrap-root role for this phase
- only the bootstrap root may request service spawn through the current syscall
  surface
- all child-service authority is then distributed explicitly as handles

This is the only remaining bootstrap exception to the otherwise
capability-oriented model. Later work should replace the role check with an
explicit bootstrap capability object.

## Service manifests

The root manager owns a built-in manifest catalog. Each manifest declares:

- stable service identity
- executable image identifier
- dependencies on other services
- service mode (`LongRunning` or `OneShot`)
- capability grants needed at startup
- restart policy

The current catalog is compiled into the root manager because there is not yet a
filesystem or package service. That keeps the schema explicit without dragging
storage policy into the kernel.

## Dependency ordering

Startup is dependency-aware but deliberately simple:

- the manifest list is arranged in a valid topological order
- each service starts only after its dependencies have reached the required
  state
- long-running services must register and become ready
- one-shot services are supervised until they complete successfully or exhaust
  their restart policy

This is enough to validate the service model without inventing a full init
framework.

## Capability distribution

The root manager does not spawn children with unrestricted authority.

Instead it:

- creates a per-service bootstrap control channel
- spawns the child with only that bootstrap endpoint
- duplicates only the specific service endpoint handles the child needs
- reduces rights on those duplicates before transfer
- closes its temporary distribution handles after the startup message is sent

In the current graph:

- `echo-service` receives a send-only logging endpoint
- `probe-service` receives a send-only logging endpoint
- `probe-service` later asks the manager for an `echo-service` endpoint through
  the registry path instead of receiving ambient access up front

## Registry and discovery

The service registry is manager-mediated, not kernel-global.

- a service registers itself by sending a public endpoint to the manager
- the manager stores that endpoint under the service identity from its manifest
- another service requests access by sending a lookup request to the manager
- the manager duplicates a rights-scoped endpoint handle into the caller's reply

This means discovery is still explicit capability distribution. There is no
ambient namespace where arbitrary services can open arbitrary peers.

## Supervision and logging

The manager tracks service phase, startup attempt count, last exit status, and
public endpoint state. It currently supports:

- lifecycle logging for start, ready, failure, restart, and stop events
- restart-on-failure for one-shot bootstrap validators
- steady-state supervision for long-running services

The example graph proves the model with three services:

- `log-service`: receives lifecycle events and emits structured logs
- `echo-service`: registers a public endpoint and serves request/reply traffic
- `probe-service`: a supervised one-shot validator that intentionally fails once
  and then succeeds after restart

## Intentionally deferred

This phase does not yet implement:

- filesystem-backed manifest loading
- persistent service state
- kernel-delivered blocking receive for userspace threads
- user fault delivery back into the manager
- dynamic package or update policy
- desktop, networking, storage, audio, or graphics services
