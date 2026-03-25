# Control Flow Foundation

## Implemented by Phase 5

- a real x86_64 IDT
- a kernel-owned GDT/TSS pair with a dedicated double-fault IST stack
- legacy PIC remapping and PIT programming for the first timer source
- Rust exception and IRQ handlers through `extern "x86-interrupt"`
- a generic interrupt/fault classification layer in `kernel/core`
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

Kernel faults are still fatal. The important addition is that the
classification path now distinguishes between kernel-origin and user-origin
faults so later phases can deliver user faults back to the owning task instead
of halting the machine.

Page fault and general protection fault handlers now log:

- fault type
- instruction pointer
- fault address when applicable
- privilege origin

## Syscall model

The initial syscall vector is `0x80`.

The syscall ABI remains intentionally modest:

- the generic kernel owns syscall number typing and dispatch tables
- the arch layer owns entry mechanics and register capture
- only ABI probe, monotonic time read, and thread-exit syscalls exist
- no handle ABI, user-buffer ABI, or service policy is baked in yet

This keeps the syscall layer extensible while userspace threads and handle
syscalls are still under construction.

## Timer and wakeup model

The current clock is a simple PIT-driven monotonic tick source.

The generic time layer supports:

- current monotonic tick reads
- one-shot and periodic deadline descriptions
- a bounded ready-to-wake queue keyed by opaque wake tokens

That queue is still explicit and mechanical, but it is no longer unused. Phase
4 binds wake tokens to blocked threads in the scheduler and routes channel
readiness into the same scheduling path.

## Deferred work

The control-flow path still does not include:

- LAPIC or HPET timer sources
- SMP interrupt routing
- fast `SYSCALL/SYSRET`
- user fault recovery
- full preemption or CPU-local run queues
