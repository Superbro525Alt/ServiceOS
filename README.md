# ServiceOS

ServiceOS is an experimental service-oriented operating system built around a
small capability-based kernel and a growing userspace platform.

Repository: <https://github.com/Superbro525Alt/ServiceOS>

The design direction is explicit:

- kernel provides mechanisms, not high-level policy
- services own platform behavior in userspace
- capabilities and handles are the primary authority model
- subsystems are kept modular so they can evolve without collapsing into a
  monolith
- early bring-up targets QEMU first, without locking long-term architecture to
  VM-only assumptions

The project is past pure kernel bring-up. It now boots a real service graph,
starts a graphical session, exposes operator tooling, supports package-driven
service activation, and has the first desktop shell and core apps.

## What Exists Today

Current foundation:

- direct UEFI boot into Rust on `x86_64`
- physical and virtual memory initialization
- reusable kernel heap allocator
- interrupt, exception, syscall, and timer foundations
- kernel object registry with handle rights and capability spaces
- channel-based IPC with explicit transfer rules
- kernel-backed blocking/wakeup behavior
- userspace process launch and flat-image loading
- per-service fault isolation by terminating the faulting task instead of
  halting the whole machine

Current userspace platform:

- root userspace bootstrap and service manager
- persisted boot-store bundle loading from the EFI system partition
- storage, log, config, console, and status services
- text shell and operator tooling
- package install, update, remove, and rollback foundation
- networking service with interface/address/route/reporting contracts
- graphics service, compositor foundation, and session service
- desktop shell with initial graphical core apps

Current graphical/product layer:

- `desktop-shell-service`
- `settings-app`
- `files-app`
- `monitor-app`

## Architecture Snapshot

ServiceOS is structured in layers:

```text
kernel
  -> root-manager
    -> foundational services
      -> platform services
        -> shell / desktop shell / apps / tools
```

Key boundaries:

- kernel owns scheduling, memory, objects, capabilities, IPC, traps, syscalls,
  and low-level device-facing primitives
- `root-manager` owns service lifecycle coordination, dependency ordering, and
  capability distribution
- storage, package, network, graphics, session, and logging policy stay in
  userspace services
- desktop shell is a product-layer service on top of graphics/session, not part
  of the compositor itself
- apps are apps, not hidden privileged platform blobs

## Current Status

The repository currently supports a real end-to-end platform flow:

1. UEFI loads the kernel image.
2. The kernel initializes memory, interrupts, syscall entry, and core object
   state.
3. The kernel launches the root userspace manager with explicit bootstrap
   authority.
4. The root manager starts a dependency-ordered service graph from persisted
   manifests.
5. Foundational and platform services come up:
   storage, config, log, console, status, package, network, graphics, session,
   shell, and desktop shell.
6. The desktop shell launches initial graphical apps through real service
   contracts.

This is still an early OS. It is not yet a polished consumer desktop, and it
does not yet include a full filesystem stack, advanced networking, audio,
compatibility runtimes, or a mature app ecosystem.

## Repository Layout

```text
.
|-- arch/
|   |-- aarch64/
|   `-- x86_64/
|-- docs/
|-- kernel/
|   |-- core/
|   `-- image/x86_64/
|-- platform/
|   |-- aarch64/raspi5/
|   `-- x86_64/qemu_virtio/
|       `-- image/
|-- shared/
|   |-- abi/
|   `-- bundle/
|-- support/xtask/
|-- userspace/
|   |-- bundles/
|   |-- catalog/
|   `-- programs/
`-- tests/
```

Important directories:

- `kernel/core`: generic kernel subsystems and object model
- `arch/x86_64`: x86_64 CPU, MMU, trap, syscall, and user-transition code
- `arch/aarch64`: aarch64 architecture scaffolding for the future Pi port
- `platform/x86_64/qemu_virtio`: UEFI, serial, framebuffer, input, and VirtIO
  backend wiring for the current QEMU target
- `platform/x86_64/qemu_virtio/image`: bootable UEFI image crate for the
  current QEMU/VirtIO platform
- `platform/aarch64/raspi5`: Raspberry Pi 5 platform scaffolding and boot image
  layout contracts
- `shared/abi`: syscall, IPC, service, graphics, network, and package ABI
- `shared/bundle`: service/package/boot-store bundle format support
- `support/xtask`: platform-aware build, image, and run orchestration
- `userspace/bundles`: manifests, config, package metadata, and static
  resources staged into the boot store
- `userspace/programs`: userspace runtime, services, desktop shell, tools, and
  apps

## Building And Running

Prerequisites:

- Rust toolchain installed
- QEMU with UEFI/OVMF available

Common commands:

```bash
cargo check --workspace
cargo test --workspace
cargo xtask build --platform qemu-virtio
cargo xtask run --platform qemu-virtio
cargo xtask image --platform raspi5
```

Useful variants:

```bash
# Headless serial-only run
QEMU_HEADLESS=1 cargo xtask run --platform qemu-virtio

# Smoke run with timeout
timeout 25 cargo xtask run --platform qemu-virtio

# Historical alias kept for convenience
cargo xtask qemu
```

Current `qemu-virtio` run defaults:

- opens a graphics window by default
- uses more than the old minimal RAM/CPU settings
- prefers KVM when available and falls back to multi-threaded TCG otherwise

Current `raspi5` image behavior:

- builds the new `arch/aarch64` and `platform/aarch64/raspi5` crates
- stages `serviceos/bootstore.bin`
- writes a Raspberry Pi boot-partition scaffold with `config.txt`
- does not claim a working native Pi kernel image yet

## What The System Can Do Right Now

From the operator shell:

- inspect services and service state
- inspect logs
- inspect stored bundle/config data
- perform package operations
- inspect network interfaces/routes/resolution state
- inspect graphics/session state
- launch desktop-aware tools and apps through the real manager/runtime path

From the graphical session:

- bring up a retained-scene desktop shell
- launch the first small set of core system apps
- exercise config, storage, status, and network-backed UI paths through real
  service contracts

## Deliberately Deferred

Not built yet:

- polished final desktop UX
- richer window management and input stack
- full filesystem/user storage semantics
- DHCP, DNS, TCP/UDP, IPv6, and richer networking policy
- audio/media stack
- network-backed package repositories and signing/trust infrastructure
- Linux or Windows compatibility layers
- full third-party app platform/toolkit ecosystem

Those are future layers on top of the current substrate, not things to force
prematurely into the kernel or early platform services.

## Documentation

High-value entry points:

- [Kernel Summary](docs/kernel-summary.md)
- [Boot Flow](docs/boot-flow.md)
- [Userspace Model](docs/userspace.md)
- [Service Model](docs/services.md)
- [Storage Foundation](docs/storage.md)
- [Package Model](docs/packages.md)
- [Networking Platform](docs/networking.md)
- [Graphics And Session Platform](docs/graphics.md)
- [Desktop Shell](docs/desktop.md)
- [Shell And Operator Environment](docs/shell.md)
- [Manifest And Bundle Schema](docs/manifests.md)
- [Future Services](docs/future-services.md)
- [Roadmap](docs/roadmap.md)

Service-specific docs:

- [Logging Service](docs/service-logging.md)
- [Config Service](docs/service-config.md)
- [Console Service](docs/service-console.md)
- [Status Service](docs/service-status.md)

## Contributing / Expectations

The codebase is being shaped as a long-lived systems project, not a one-off OS
toy. Changes should preserve these constraints:

- keep kernel and userspace boundaries clean
- prefer explicit capabilities over ambient authority
- keep service contracts durable and replaceable
- avoid baking bring-up shortcuts into long-term public interfaces
- improve observability without leaking temporary milestone naming into runtime
  behavior

## License

See the repository license metadata and workspace manifests for current license
information.
