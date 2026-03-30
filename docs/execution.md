# Execution Model

## Process and thread model

The current task object is the process-equivalent container.

A task owns:

- an optional address-space identifier
- a capability space
- a set of member threads
- a role such as bootstrap root or system service

A thread owns:

- an owning task identifier
- an execution mode: `Kernel` or `User`
- an execution context description: entry instruction pointer and stack pointer
- a scheduling context with a simple round-robin quantum
- an execution state and optional wait target

## Scheduler model

The current scheduler is deliberately simple:

- single-core
- round-robin
- one current thread
- one runnable queue
- explicit blocked queues for timer waits and channel-receive waits

This is enough to make state transitions real without baking in policy that
would fight later SMP or richer userspace work.

## State transitions

Threads move through these states:

- `Constructing`
- `Suspended`
- `Runnable`
- `Running`
- `Blocked`
- `Dying`

Important transitions in the current implementation:

- newly created threads are registered with the scheduler and start suspended
- making a thread runnable places it on the run queue
- yielding the current thread rotates execution to the next runnable thread
- blocking on channel receive moves the current thread to the channel wait set
- blocking on a timer arms a wake token and moves the current thread to the
  timer wait set
- IPC send makes one blocked receiver runnable again
- timer expiry makes one blocked timer waiter runnable again

## Userspace implications

This model is now exercised by a real service platform:

- user tasks carry real address-space attachment points
- `ThreadMode::User` reaches ring 3 in normal bring-up
- the root manager and foundational services run on the same scheduler
- later blocking syscalls can reuse the same scheduler APIs

The runtime model is also less bootstrap-bound than before:

- the root manager can now launch stored executable images through the storage
  service and a manager-owned image-loading path
- dynamic image launch no longer has to be hardwired to built-in image ids only
- user tasks now report `Faulted` as a distinct terminal state instead of
  collapsing every failure into a generic exit

That keeps loading policy in the manager/runtime layer rather than scattering
it into shell or app launch code.

## Roadmap note

Open scheduler, fault-handling, and executable-loading follow-on work is
tracked centrally in [docs/roadmap.md](roadmap.md). This page intentionally
stays focused on the current execution model and implemented behavior.
