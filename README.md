# ServiceOS Foundation

The repository now covers the early kernel foundation plus the first durable
userspace platform layer:

- direct UEFI boot into Rust on `x86_64`
- real UEFI memory-map capture
- early physical frame allocation
- initial x86_64 paging foundation
- mapped kernel heap bootstrap
- interrupt descriptor table and GDT/TSS bring-up
- syscall dispatch on a dedicated software-interrupt vector
- unified kernel object registry
- per-task capability spaces and handle rights
- channel-based IPC with explicit transfer rights
- a schedulable thread model
- user address-space construction and ring-3 entry
- a staged boot-store image loaded from the EFI system partition
- a root userspace service manager launched by the kernel
- dependency-aware service startup from persisted manifests
- explicit capability distribution from root into child services
- a refined manager-mediated service registry
- foundational userspace services for storage, logging, configuration, console
  I/O, and system status
- a text-first shell service and transient tool launch path for in-system
  operation
- a package service with install, update, remove, and rollback coordination
- a userspace networking platform service with explicit packet-interface,
  address, route, and name-resolution contracts
- host-side unit tests for the core kernel semantics

The system remains intentionally early. It does not attempt desktop,
audio, graphics, or compatibility stacks yet.

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
|   |-- bundles/
|   |-- catalog/
|   `-- programs/
`-- tests/
```

- `shared/abi`: shared syscall, IPC, capability-transfer, and service identity
  ABI
- `shared/bundle`: boot-store archive and service-manifest schema
- `kernel/core`: generic kernel bootstrap and subsystem foundations
- `kernel/arch/x86_64`: x86_64 boot, CPU, serial, paging, trap, and user-entry
  implementation
- `kernel/image/x86_64`: bootable UEFI kernel image entry point
- `support/xtask`: host-side build and QEMU runner logic
- `userspace/bundles`: persisted manifests, config, and static resources packed
  into the boot store
- `userspace/catalog`: host-side boot-store builder and flat-image packer
- `userspace/programs`: freestanding userspace runtime plus the root manager and
  platform services

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
loads a boot-store image from the EFI system partition, initializes the memory
substrate, installs an x86_64 GDT/TSS/IDT, enables PIC/PIT-driven timer
interrupts, and then hands off to a real userspace root service manager.

The current platform layer provides:

- a registry-backed kernel object namespace
- task-local capability spaces
- channel endpoints as first-class kernel objects
- per-handle transfer-right control during IPC
- dedicated user address-space roots with shared kernel mappings
- a flat-image loader backed by the staged boot store
- a root service manager that starts a named dependency-ordered service graph
  from persisted manifests
- explicit startup grants from root into child services
- manager-mediated lookup with per-service lookup permissions
- a `storage-service` that opens persisted objects as explicit blob
  capabilities
- a structured `log-service` that forwards through a `console-service`
- a `config-service` backed by persisted configuration data
- a long-running `status-service` that depends on log, config, and console and
  consumes a startup-granted resource blob
- a `shell-service` that owns the first operator session and command surface
- a manager-mediated transient tool launch path validated by `sysinfo-tool`
- a `package-service` that activates repository-backed service packages through
  the root manager
- a package repository format for versioned service bundles such as the current
  `announce-service` package
- a `network-service` that owns interface state, static IPv4 configuration,
  route reporting, static host resolution, and ICMP probe handling behind an
  explicit service contract

The current syscall surface is intentionally small, but it is now enough for a
real service platform:

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
- `12`: kernel memory-object read for boot-rooted storage hydration
- `13`: raw debug-console byte read for console input polling
- `14`: raw debug-console byte write for session output
- `15`: packet-interface status query
- `16`: packet-interface frame receive
- `17`: packet-interface frame transmit

See [docs/kernel-summary.md](/home/paulh/os-dev/docs/kernel-summary.md),
[docs/boot-flow.md](/home/paulh/os-dev/docs/boot-flow.md),
[docs/networking.md](/home/paulh/os-dev/docs/networking.md),
[docs/storage.md](/home/paulh/os-dev/docs/storage.md),
[docs/userspace.md](/home/paulh/os-dev/docs/userspace.md),
[docs/services.md](/home/paulh/os-dev/docs/services.md),
[docs/shell.md](/home/paulh/os-dev/docs/shell.md),
[docs/packages.md](/home/paulh/os-dev/docs/packages.md),
[docs/manifests.md](/home/paulh/os-dev/docs/manifests.md),
[docs/service-logging.md](/home/paulh/os-dev/docs/service-logging.md),
[docs/service-config.md](/home/paulh/os-dev/docs/service-config.md),
[docs/service-console.md](/home/paulh/os-dev/docs/service-console.md),
[docs/service-status.md](/home/paulh/os-dev/docs/service-status.md),
[docs/future-services.md](/home/paulh/os-dev/docs/future-services.md), and
[docs/roadmap.md](/home/paulh/os-dev/docs/roadmap.md).
