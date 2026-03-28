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
- package-delivered developer tooling foundation with toolchain, workspace, and
  cross-target build workflows
- networking service with dynamic IPv4, DNS-backed resolution, ICMP, and
  outbound TCP stream contracts
- audio service with explicit endpoint and playback-stream contracts
- graphics service, compositor foundation, and session service
- desktop shell with initial graphical core apps
- writable storage namespaces with scoped directory and file capabilities
- manager-owned stored-image launching through the storage path

Current graphical/product layer:

- `desktop-shell-service`
- `settings-app`
- `files-app`
- `monitor-app`
- `terminal-app`

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
   storage, config, log, console, status, package, network, audio, graphics,
   session, shell, and desktop shell.
6. The desktop shell launches initial graphical apps through real service
   contracts.

This is still an early OS. It is not yet a polished consumer desktop, and it
does not yet include a full filesystem stack, richer audio/media,
compatibility runtimes, or a mature app ecosystem.

## Repository Layout

```text
.
|-- arch/
|   |-- aarch64/
|   `-- x86_64/
|-- docs/
|-- kernel/
|   `-- core/
|-- platform/
|   |-- aarch64/raspi5/
|   |   `-- image/
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
- `arch/aarch64`: aarch64 CPU and early bring-up primitives for the Pi port
- `platform/x86_64/qemu_virtio`: UEFI, serial, framebuffer, input, and VirtIO
  backend wiring for the current QEMU target
- `platform/x86_64/qemu_virtio/image`: bootable UEFI image crate for the
  current QEMU/VirtIO platform
- `platform/aarch64/raspi5`: Raspberry Pi 5 firmware, DTB, and UART bring-up
  code
- `platform/aarch64/raspi5/image`: native Raspberry Pi 5 `kernel8.img` crate
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

- builds `arch/aarch64`, `platform/aarch64/raspi5`, and the native Pi image
  crate
- emits a real `kernel8.img` plus `serviceos/serviceos-kernel.elf`
- stages `serviceos/bootstore.bin` and embeds the same boot-store into the Pi
  kernel image for early userspace bootstrap
- parses the Raspberry Pi DTB at boot to discover memory and the debug UART
- brings up the `aarch64` MMU, EL1 trap/syscall path, and EL0 user-thread
  execution path
- initializes `kernel/core` and launches a serial-first userspace graph on the
  Pi 5 debug UART
- does not yet provide Raspberry Pi-native graphics, pointer/keyboard, network,
  or writable storage backends

## What The System Can Do Right Now

From the operator shell:

- inspect services and service state
- inspect logs
- inspect stored bundle/config data
- create and update files in writable namespaces through the real storage
  service
- perform package operations
- install optional runtime support packages
- install optional developer tooling support packages
- create compatibility/runtime environments and inspect their mounts, variables,
  and runs
- launch runtime-hosted workloads through the real manager/runtime path
- inspect packaged toolchains and workspaces
- run native, Linux, and Windows cross-target builds through the real
  developer-service path
- inspect build jobs and exported artifact handles
- inspect network interfaces/routes/resolution state
- perform practical outbound network checks and fetches through the real
  network-service path
- launch desktop apps and post notifications through the desktop contract
- launch stored flat images through the manager-owned loader path
  networking service
- inspect audio endpoints and streams and run diagnostic playback through the
  real audio service
- inspect graphics/session state
- launch desktop-aware tools and apps through the real manager/runtime path

From the graphical session:

- bring up a retained-scene desktop shell
- launch the first small set of core system apps
- launch a graphical terminal window that hosts the shared shell/runtime stack
- exercise config, storage, status, and network-backed UI paths through real
  service contracts

## Deliberately Deferred

Not built yet:

- polished final desktop UX
- richer window management and input stack
- full filesystem/user storage semantics
- UDP, IPv6, inbound/listening sockets, and richer networking policy
- richer PCM/capture audio pipelines, mixing, and broader media policy
- network-backed package repositories and signing/trust infrastructure
- Linux ABI compatibility, Windows `.exe` support, and stronger runtime
  sandboxing
- richer language ecosystems, remote macOS build/sign workflows, and stronger
  build-worker sandboxing
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
- [Audio Platform](docs/audio.md)
- [Compatibility And Runtime Foundations](docs/runtime.md)
- [Graphics And Session Platform](docs/graphics.md)
- [Desktop Shell](docs/desktop.md)
- [Graphical Terminal](docs/terminal.md)
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
