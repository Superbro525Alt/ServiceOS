# ServiceOS Kernel Foundation

The repository now covers Phase 2 of the kernel bring-up:

- direct UEFI boot into Rust on `x86_64`
- real UEFI memory-map capture
- early physical frame allocation
- initial x86_64 paging foundation
- mapped kernel heap bootstrap
- interrupt descriptor table and GDT/TSS bring-up
- exception and fault reporting structure
- PIC/PIT-backed monotonic tick and deadline wakeups
- syscall dispatch groundwork on a dedicated software-interrupt vector

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

Phase 2 boots cleanly under QEMU, exits UEFI boot services, captures the memory
map, initializes the memory substrate, installs an x86_64 GDT/TSS/IDT, enables
PIC/PIT-driven timer interrupts, maintains a monotonic tick source with a small
deadline wakeup queue, and routes exceptions through explicit Rust handlers.

The current scheduler and userspace model still do not exist. The timer and
wakeup code only provides the low-level mechanism that later task and IPC waits
will consume. The syscall dispatcher is installed on vector `0x80`, but it is
still phase-appropriate groundwork rather than a full user ABI.

See [docs/architecture.md](/home/paulh/os-dev/docs/architecture.md),
[docs/boot-flow.md](/home/paulh/os-dev/docs/boot-flow.md),
[docs/control-flow.md](/home/paulh/os-dev/docs/control-flow.md),
[docs/memory.md](/home/paulh/os-dev/docs/memory.md),
[docs/subsystems.md](/home/paulh/os-dev/docs/subsystems.md), and
[docs/roadmap.md](/home/paulh/os-dev/docs/roadmap.md).
