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

- `arch/x86_64` owns descriptor tables, low-level x86 entry mechanics, and the
  current syscall/trap ABI
- `kernel/core` owns trap accounting, fault disposition, syscall dispatch,
  timer queue state, and deferred scheduler wake handling

Temporary detail:

- PIC/PIT programming and IRQ acknowledgement still live in `arch/x86_64`
  today because the current x86 target is only `qemu-virtio`; that
  platform-specific part is now explicitly documented and can move down into a
  future x86 platform crate without changing `kernel/core`

## Fault model

Kernel faults are still fatal. User-origin faults now terminate only the
faulting task and return control to the executor, which lets the root manager
observe service failure and apply restart policy. Richer fault upcalls and
recovery contracts remain later work.

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
- bootstrap-capability-gated service spawn
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

## Roadmap note

Open scheduler and interrupt follow-on work is tracked centrally in
[docs/roadmap.md](roadmap.md). This page intentionally stays focused on the
current control-flow model and implemented behavior.
