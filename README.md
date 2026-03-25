# ServiceOS Kernel Foundation

Phase 0 establishes the repository shape, boot path scaffold, and subsystem
boundaries for a service-oriented, capability-oriented operating system.

This repository intentionally stops short of implementing a real kernel. The
goal is to create a foundation that can evolve without forcing early policy
decisions into the kernel.

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

- `kernel/core`: generic kernel interfaces and subsystem boundaries
- `kernel/arch/x86_64`: architecture-specific bring-up helpers and hardware stubs
- `kernel/image/x86_64`: bootable kernel image entry point
- `support/xtask`: host-side build and QEMU runner logic

## Commands

```bash
cargo xtask build
cargo xtask qemu
```

Optional release build:

```bash
cargo xtask build --release
```

## Phase 0 scope

- Clean repository and workspace structure
- Minimal boot flow into Rust kernel code
- Generic subsystem modules with documented placeholder types
- Architecture and roadmap documentation
- Host-side tooling for EFI staging and QEMU execution

See [docs/architecture.md](/home/paulh/os-dev/docs/architecture.md),
[docs/boot-flow.md](/home/paulh/os-dev/docs/boot-flow.md),
[docs/subsystems.md](/home/paulh/os-dev/docs/subsystems.md), and
[docs/roadmap.md](/home/paulh/os-dev/docs/roadmap.md) for the design baseline.
