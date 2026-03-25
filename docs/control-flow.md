# Control Flow Foundation

## What exists now

- a real x86_64 IDT
- a kernel-owned GDT/TSS pair with a dedicated double-fault IST stack
- legacy PIC remapping and PIT programming for the first timer source
- Rust exception and IRQ handlers through `extern "x86-interrupt"`
- a generic interrupt and fault classification layer in `kernel/core`
- a monotonic tick source with deadline wakeup bookkeeping
- a dedicated software-interrupt syscall vector at `0x80`
- a scheduler that consumes timer wakeups and IPC-readiness events
- a ring-3 launch path that returns through the same kernel control-flow spine

## Trap and interrupt flow

```text
CPU event
  -> x86_64 IDT entry
    -> arch/x86_64 interrupt or exception handler
      -> kernel/core interrupt classification
        -> timer tick accounting, fatal fault reporting, or syscall dispatch
```

The boundary remains explicit:

- `kernel/arch/x86_64` owns descriptor tables, PIC/PIT programming, IRQ
  acknowledgement, and low-level entry mechanics
- `kernel/core` owns trap accounting, fault disposition, syscall dispatch,
  timer queue state, and deferred scheduler wake handling

## Fault model

Kernel faults are still fatal. The important distinction already in place is
kernel-origin versus user-origin classification so later work can deliver user
faults back to the owning task instead of halting the machine.

## Syscall model

The initial syscall vector is `0x80`.

The current ABI is intentionally small:

- ABI probe
- monotonic time read
- current-thread exit
- cooperative yield
- debug log write
- channel create/send/receive
- handle duplicate and close
- bootstrap-only service spawn
- task status query

This is enough for the root manager and foundational services without baking
high-level service policy into the kernel.

## Timer and wakeup model

The current clock is a PIT-driven monotonic tick source.

The generic time layer supports:

- current monotonic tick reads
- one-shot deadline descriptions
- wake tokens consumed by the scheduler

That foundation now directly drives the long-running `status-service`.

## Deferred work

- LAPIC or HPET timer sources
- SMP interrupt routing
- fast `SYSCALL/SYSRET`
- user fault recovery
- full preemption or CPU-local run queues
