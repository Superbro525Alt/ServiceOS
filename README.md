# ServiceOS Kernel Foundation

The repository now covers Phase 1 of the kernel bring-up:

- direct UEFI boot into Rust on `x86_64`
- real UEFI memory-map capture
- early physical frame allocation
- initial x86_64 paging foundation
- mapped kernel heap bootstrap
- address-space layout groundwork for later isolation

The system remains intentionally early. It does not attempt scheduling,
userspace launch, IPC policy, drivers, filesystems, networking, audio, or GUI.

## Initial target

- Architecture: `x86_64`
- Bring-up environment: `QEMU`
- Primary firmware path: `UEFI`
- Primary implementation language: `Rust`
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

Phase 1 boots cleanly under QEMU, exits UEFI boot services, captures the memory
map, initializes a conservative frame allocator from conventional memory,
modifies the active page tables to map a dedicated high-half heap region, and
records the active kernel address-space root for later growth.

The current heap allocator is intentionally bootstrap-grade: it provides a
simple monotonic kernel allocation foundation without pretending to solve the
final object-allocation problem yet.

See [docs/architecture.md](/home/paulh/os-dev/docs/architecture.md),
[docs/boot-flow.md](/home/paulh/os-dev/docs/boot-flow.md),
[docs/memory.md](/home/paulh/os-dev/docs/memory.md),
[docs/subsystems.md](/home/paulh/os-dev/docs/subsystems.md), and
[docs/roadmap.md](/home/paulh/os-dev/docs/roadmap.md).
