# ServiceOS Kernel Foundation

The repository now covers Phase 4 of the kernel bring-up:

- direct UEFI boot into Rust on `x86_64`
- real UEFI memory-map capture
- early physical frame allocation
- initial x86_64 paging foundation
- mapped kernel heap bootstrap
- interrupt descriptor table and GDT/TSS bring-up
- exception and fault reporting structure
- PIC/PIT-backed monotonic tick and deadline wakeups
- syscall dispatch groundwork on a dedicated software-interrupt vector
- unified kernel object registry
- per-task capability spaces and handle rights
- channel-based IPC with capability transfer
- a schedulable thread model
- a round-robin scheduler foundation
- timer and IPC wakeups routed into task state transitions

The system remains intentionally early. It does not attempt scheduling,
userspace launch, IPC policy, drivers, filesystems, networking, audio, or GUI.

## Initial target

- Architecture: `x86_64`
- Bring-up environment: `QEMU`
- Primary firmware path: `UEFI`
- Primary implementation language: `Rust`
- Toolchain: pinned `nightly` for `extern "x86-interrupt"` support in the
  x86_64 trap path
- Boot handoff: direct UEFI entry using
  [`uefi`](https://docs.rs/uefi/latest/uefi/)

## Workspace layout

```text
.
|-- docs/
|-- kernel/
|   |-- arch/x86_64/
|   |-- core/
|   `-- image/x86_64/
|-- support/xtask/
`-- tests/
```

- `kernel/core`: generic kernel bootstrap and subsystem foundations
- `kernel/arch/x86_64`: x86_64 boot, CPU, serial, and paging implementation
- `kernel/image/x86_64`: bootable UEFI kernel image entry point
- `support/xtask`: host-side build and QEMU runner logic

## Commands

```bash
cargo check --workspace
cargo xtask build
cargo xtask qemu
```

Optional release build:

```bash
cargo xtask build --release
```

## Current state

Phase 4 boots cleanly under QEMU, exits UEFI boot services, captures the memory
map, initializes the memory substrate, installs an x86_64 GDT/TSS/IDT, enables
PIC/PIT-driven timer interrupts, and then brings up a kernel object model with:

- a registry-backed object namespace
- bootstrap and service-task capability spaces
- channel endpoints as first-class kernel objects
- capability duplication and transfer with rights reduction
- explicit handle close and weak-registry garbage collection
- a bootstrap kernel thread plus service threads registered with the scheduler
- channel-receive blocking and timer blocking feeding back into scheduling

Userspace service launch still does not exist, but the kernel now has the basic
execution model it needs for it: process-equivalent task objects, schedulable
threads, explicit blocking states, and wakeup paths driven by IPC and timers.
The syscall dispatcher on vector `0x80` remains an early ABI boundary rather
than a complete user ABI.

See [docs/architecture.md](/home/paulh/os-dev/docs/architecture.md),
[docs/boot-flow.md](/home/paulh/os-dev/docs/boot-flow.md),
[docs/control-flow.md](/home/paulh/os-dev/docs/control-flow.md),
[docs/execution.md](/home/paulh/os-dev/docs/execution.md),
[docs/memory.md](/home/paulh/os-dev/docs/memory.md),
[docs/objects.md](/home/paulh/os-dev/docs/objects.md),
[docs/subsystems.md](/home/paulh/os-dev/docs/subsystems.md), and
[docs/roadmap.md](/home/paulh/os-dev/docs/roadmap.md).
