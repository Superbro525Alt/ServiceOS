# ServiceOS Foundation

The repository now covers the full early-kernel foundation plus the first
durable userspace platform layer:

- direct UEFI boot into Rust on `x86_64`
- real UEFI memory-map capture
- early physical frame allocation
- initial x86_64 paging foundation
- mapped kernel heap bootstrap
- interrupt descriptor table and GDT/TSS bring-up
- exception and fault reporting structure
- PIC/PIT-backed monotonic tick and deadline wakeups
- syscall dispatch on a dedicated software-interrupt vector
- unified kernel object registry
- per-task capability spaces and handle rights
- channel-based IPC with explicit transfer rights
- a schedulable thread model
- a round-robin scheduler foundation
- user address-space construction and ring-3 entry
- a built-in userspace image catalog
- a root userspace service manager launched by the kernel
- dependency-aware service startup from manifests
- explicit capability distribution from root into child services
- a refined manager-mediated service registry
- foundational userspace services for logging, configuration, console I/O, and
  system status
- host-side unit tests for the core kernel semantics

The system remains intentionally early. It does not attempt desktop, package,
networking, storage, audio, graphics, or compatibility stacks yet.

## Initial target

- Architecture: `x86_64`
- Bring-up environment: `QEMU`
- Primary firmware path: `UEFI`
- Primary implementation language: `Rust`
- Toolchain: pinned `nightly` for `extern "x86-interrupt"` support in the
  x86_64 trap path

## Workspace layout

```text
.
|-- docs/
|-- shared/
|-- kernel/
|   |-- arch/x86_64/
|   |-- core/
|   `-- image/x86_64/
|-- support/xtask/
|-- userspace/
|   |-- catalog/
|   `-- programs/
`-- tests/
```

- `shared/abi`: shared syscall, IPC, capability-transfer, and service identity
  ABI
- `kernel/core`: generic kernel bootstrap and subsystem foundations
- `kernel/arch/x86_64`: x86_64 boot, CPU, serial, paging, trap, and user-entry
  implementation
- `kernel/image/x86_64`: bootable UEFI kernel image entry point
- `support/xtask`: host-side build and QEMU runner logic
- `userspace/catalog`: host-built catalog of bootable flat userspace images
- `userspace/programs`: freestanding userspace runtime plus the root manager and
  foundational services

## Commands

```bash
cargo check --workspace
cargo test --workspace
cargo xtask build
cargo xtask qemu
```

For smoke testing, keep `qemu` under a timeout because the system now stays
alive under the service manager:

```bash
timeout 20 cargo xtask qemu
```

## Current state

The system boots under QEMU, exits UEFI boot services, captures the memory map,
initializes the memory substrate, installs an x86_64 GDT/TSS/IDT, enables
PIC/PIT-driven timer interrupts, and then hands off to a real userspace root
service manager. The current platform layer provides:

- a registry-backed kernel object namespace
- task-local capability spaces
- channel endpoints as first-class kernel objects
- per-handle transfer-right control during IPC
- dedicated user address-space roots with shared kernel mappings
- a flat-image loader and bootstrap user stack
- a root service manager that starts a named dependency-ordered service graph
- explicit startup grants from root into child services
- manager-mediated lookup with per-service lookup permissions
- a structured `log-service` that forwards through a `console-service`
- a `config-service` with a small typed configuration schema
- a long-running `status-service` that depends on log, config, and console

The current syscall surface is still intentionally small, but it is now enough
for a real service platform:

- `0`: ABI version probe
- `1`: monotonic tick read
- `2`: current-thread exit
- `3`: cooperative yield
- `4`: kernel-routed debug log write
- `5`: channel creation
- `6`: channel send
- `7`: channel receive
- `8`: handle duplication with rights reduction
- `9`: handle close
- `10`: bootstrap-only service spawn
- `11`: task status query

See [docs/kernel-summary.md](/home/paulh/os-dev/docs/kernel-summary.md),
[docs/boot-flow.md](/home/paulh/os-dev/docs/boot-flow.md),
[docs/userspace.md](/home/paulh/os-dev/docs/userspace.md),
[docs/services.md](/home/paulh/os-dev/docs/services.md),
[docs/manifests.md](/home/paulh/os-dev/docs/manifests.md),
[docs/service-logging.md](/home/paulh/os-dev/docs/service-logging.md),
[docs/service-config.md](/home/paulh/os-dev/docs/service-config.md),
[docs/service-console.md](/home/paulh/os-dev/docs/service-console.md),
[docs/service-status.md](/home/paulh/os-dev/docs/service-status.md),
[docs/future-services.md](/home/paulh/os-dev/docs/future-services.md), and
[docs/roadmap.md](/home/paulh/os-dev/docs/roadmap.md).
