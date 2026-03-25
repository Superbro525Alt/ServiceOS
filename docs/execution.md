# Execution Model

## Phase 5 scope

Phase 5 keeps the Phase 4 scheduler model and adds the first real user thread
launch path on top of it.

## Process and thread model

The current task object is the process-equivalent container.

A task owns:

- an optional future address-space identifier
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
would fight later SMP or userspace work.

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

## Wakeup model

Phase 4 integrates two real wakeup paths:

- timer interrupts feed the monotonic clock and produce `WakeEvent`s
- channel send operations notify the scheduler that a receive waiter can run

The scheduler consumes both and turns them into runnable threads. The current
boot demo exercises both paths in one sequence.

## Userspace bootstrap implications

This model is now exercised by a real bootstrap path:

- the task object now carries a real user address-space attachment point in the
  first user launch path
- the `ThreadMode::User` variant now reaches ring 3 in the boot demo
- blocking and wake transitions are expressed independently of service policy
- later syscall handlers can block the current thread through the same
  scheduler APIs used by the boot demo

## Still deferred

Phase 5 still does not include:

- actual context switching of CPU register state
- preemptive time-slice enforcement
- SMP scheduling
- user fault delivery back into the owning task
- a general executable loader or root service manager policy
