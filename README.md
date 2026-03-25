# ServiceOS Foundation

The repository now covers the full early-kernel foundation plus the first real
userspace platform layer:

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
- user address-space construction and ring-3 entry
- a built-in userspace image catalog
- a root userspace service manager launched by the kernel
- dependency-aware service startup from manifests
- explicit capability distribution from root into child services
- manager-mediated service registration and discovery
- supervised example services for logging, request/reply IPC, and bootstrap
  validation
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
- Boot handoff: direct UEFI entry using
  [`uefi`](https://docs.rs/uefi/latest/uefi/)

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

- `shared/abi`: syscall, IPC, handle, and service identity ABI shared between
  kernel and userspace
- `kernel/core`: generic kernel bootstrap and subsystem foundations
- `kernel/arch/x86_64`: x86_64 boot, CPU, serial, paging, trap, and user-entry
  implementation
- `kernel/image/x86_64`: bootable UEFI kernel image entry point
- `support/xtask`: host-side build and QEMU runner logic
- `userspace/catalog`: host-built catalog of bootable flat userspace images
- `userspace/programs`: freestanding userspace runtime plus the root manager
  and example services

## Commands

```bash
cargo check --workspace
cargo test --workspace
cargo xtask build
cargo xtask qemu
```

For smoke testing, keep `qemu` under a timeout because the system now stays
alive under the root service manager:

```bash
timeout 20 cargo xtask qemu
```

Optional QEMU debugging:

```bash
QEMU_EXTRA_ARGS="-d int -D target/qemu-int.log" cargo xtask qemu
```

Optional release build:

```bash
cargo xtask build --release
```

## Current state

The system now boots under QEMU, exits UEFI boot services, captures the memory
map, initializes the memory substrate, installs an x86_64 GDT/TSS/IDT, enables
PIC/PIT-driven timer interrupts, and then hands off to a real userspace root
service manager. The current platform layer provides:

- a registry-backed object namespace
- bootstrap and service-task capability spaces
- channel endpoints as first-class kernel objects
- capability duplication and transfer with rights reduction
- explicit handle close and weak-registry garbage collection
- a bootstrap kernel thread plus service threads registered with the scheduler
- channel-receive and timer wakeups integrated into task state transitions
- a dedicated user page-table root with shared kernel mappings
- a flat-image loader and bootstrap user stack
- userspace threads that execute in ring 3 and use the syscall ABI on vector
  `0x80`
- a root service manager that starts a dependency-ordered service graph
- limited capability grants from the root manager into child services
- manager-mediated service discovery instead of ambient global lookup
- restart supervision for a one-shot bootstrap validation service
- explicit capability-handle exhaustion checks and bounded IPC queues
- host-side unit coverage for capabilities, IPC, object lifetime, memory,
  scheduler transitions, syscalls, and user-image parsing invariants

The current syscall surface is still intentionally small, but it is now enough
for a real service bootstrap:

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
[docs/future-services.md](/home/paulh/os-dev/docs/future-services.md),
[docs/architecture.md](/home/paulh/os-dev/docs/architecture.md),
[docs/boot-flow.md](/home/paulh/os-dev/docs/boot-flow.md),
[docs/control-flow.md](/home/paulh/os-dev/docs/control-flow.md),
[docs/execution.md](/home/paulh/os-dev/docs/execution.md),
[docs/memory.md](/home/paulh/os-dev/docs/memory.md),
[docs/objects.md](/home/paulh/os-dev/docs/objects.md),
[docs/services.md](/home/paulh/os-dev/docs/services.md),
[docs/manifests.md](/home/paulh/os-dev/docs/manifests.md),
[docs/subsystems.md](/home/paulh/os-dev/docs/subsystems.md),
[docs/userspace.md](/home/paulh/os-dev/docs/userspace.md), and
[docs/roadmap.md](/home/paulh/os-dev/docs/roadmap.md).
