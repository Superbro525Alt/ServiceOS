# Roadmap

This file is the single source of truth for deferred and future work.
Other docs in `docs/` should describe the current implemented state and point
back here instead of carrying their own deferred-work lists.

## Kernel foundation completed

- firmware handoff, memory discovery, paging, and kernel heap bootstrap
- reusable kernel heap allocator with free-list reclamation
- interrupt, exception, syscall, and timer foundations
- kernel object model, capabilities, and IPC
- scheduler, task model, and userspace entry
- hardening, tests, and architecture cleanup

## Root bootstrap completed

- root userspace service manager
- persisted boot-store manifest index
- dependency-ordered startup
- capability-scoped startup grants
- manager-mediated registration and discovery

## Foundational services completed

- `storage-service`
- `console-service`
- `config-service`
- `log-service`
- `status-service`
- `network-service`
- explicit lookup policy and per-handle transfer-right control

## Platform tooling completed

- `shell-service` operator environment
- `package-service` install, update, rollback, and activation coordination

## Architecture and platform abstraction completed

- explicit `kernel/core`, `arch/<isa>`, and `platform/<platform>` layering
- `arch/x86_64` as the active ISA crate
- `platform/x86_64/qemu_virtio` for UEFI, serial, framebuffer, input, and
  VirtIO backend wiring
- `arch/aarch64` and `platform/aarch64/raspi5` as first-class targets with a
  native Raspberry Pi 5 image and serial-first userspace bootstrap
- platform-first `xtask` build, image, and run selection
- normalized `BootInfo` handoff into generic kernel initialization

## Graphics/session foundation completed

- kernel display-output object
- `graphics-service` output, surface, and composition ownership
- `session-service` focus and graphical session ownership
- shell/operator inspection of outputs, surfaces, and sessions

## Desktop shell foundation completed

- `desktop-shell-service` graphical shell chrome and launcher surface ownership
- manager-mediated graphical app launch and focus handling
- first core app set: `settings-app`, `files-app`, `monitor-app`
- serial-shell inspection of desktop state through the desktop-shell contract

## Desktop interaction completed

- window focus, z-order, move, resize, minimize, restore, and close handling in
  `desktop-shell-service`
- maximize handling in `desktop-shell-service`
- app-control channel for desktop-driven focus, resize, close, pointer, key,
  and text events
- physical input routing from the kernel input-source object through
  `session-service` into the desktop interaction contract
- shell/operator inspection and control of desktop windows through the
  desktop-shell contract

## Compatibility/runtime foundation completed

- package-delivered `runtime-service` for explicit compatibility/runtime
  environments
- environment creation, inspection, run launch, and teardown through a real
  userspace service contract
- explicit mount and variable mapping for the first `posix` runtime profile
- manager-mediated launch of runtime-hosted workloads instead of shell-owned
  compatibility shortcuts
- shared shell/terminal operator integration for runtime inspection and launch

## Developer tooling foundation completed

- package-delivered `developer-service` for toolchain, workspace, build-job,
  and artifact management
- packaged toolchain descriptors for native, Linux, Windows, and honest
  remote-only macOS target metadata
- packaged workspace descriptors and sample source payloads for the first
  cross-target workflow
- manager-mediated launch of transient `cross-builder-tool` workers instead of
  shell-owned build shortcuts
- shared shell/terminal operator integration for toolchain inspection, build
  submission, job inspection, and artifact export

## Open work

### Platform and hardware follow-on

- add Raspberry Pi 5 framebuffer, graphical input, networking, writable
  boot-store, and audio backends behind the current
  `platform/aarch64/raspi5` contracts
- expand Raspberry Pi 5 beyond the current serial-first bootstrap into the
  normal graphical and network-backed service graph
- split the remaining x86 PC interrupt-controller details out of
  `arch/x86_64` if a second x86 platform target is added

### Kernel scheduling, memory, and fault handling

- complete real CPU register context switching between unrelated kernel threads
  instead of the current bootstrap-oriented path
- add preemptive time-slice enforcement, CPU-local run queues, and SMP
  scheduling and interrupt routing
- add alternate x86 timer and interrupt sources such as LAPIC and HPET where
  they improve wake behavior and scheduling fidelity
- switch from the current boot-derived paging path to fully kernel-owned page
  tables and a fuller physical direct map
- add fast `SYSCALL/SYSRET` on x86_64 alongside the current interrupt-gate
  syscall path
- extend the current fault-state propagation, manager supervision, and desktop
  fault surfacing into richer user-fault upcalls and recovery policy
- expose richer VM and memory-mapping syscalls needed for stronger runtimes,
  DMA-safe engines, and future process models
- generalize memory-object mapping and shared-memory IPC beyond the current
  graphics-oriented use, and add richer object inspection and wait primitives

### Storage, filesystem, packages, and configuration

- replace the current in-memory writable overlay with persistent writable
  backing
- add block-device service contracts, mount management, namespace composition,
  and broader application-facing file and directory protocols
- add broader user-home and storage policy, plus writable project/workspace
  directories and persistent build outputs
- add network-backed repositories, signed feeds, trust metadata, writable
  install roots, install journals, and rollback policy for packages
- add dynamic service installation, on-demand activation policy, and richer
  health-check definitions in manifests and package flows
- add GUI package management and software-center style package UX
- add whole-system image update workflows on top of the package/update
  foundation
- add namespaced service configuration trees, write and update policy, and
  schema validation and migration

### Execution, loading, compatibility, and runtimes

- grow the current stored flat-image loader into ELF and other richer
  executable formats, dependency loading, and broader runtime policy
- add a general process loader for user-supplied images instead of only
  manager-owned stored images
- grow the current compatibility/runtime foundation beyond hosted `posix`
  environments into Linux-oriented ABI expansion, arbitrary ELF execution,
  richer runtime packaging, and desktop launch UX for runtime-hosted apps
- add explicit capability grants for network, graphics, input, and audio to
  compatibility workloads
- add Windows runtime support and broader cross-platform application execution
- add stronger sandboxing and container-style isolation for compatibility
  workloads

### Networking

- grow the current networking surface beyond DHCP/DNS/ICMP/outbound TCP into
  UDP, inbound/listening transports, richer routing, multi-interface policy,
  and IPv6
- add long-lived resolver caching, richer DNS record support, firewalling, and
  broader network policy
- move beyond copied-frame transport into packet-buffer sharing and zero-copy
  networking
- add richer NIC interrupt models such as MSI/MSI-X, additional virtual
  backends, and real NIC driver hosts beyond current QEMU/VirtIO

### Graphics, input, desktop, terminal, and shell

- grow the current mapped-buffer graphics path into damage-tracked multi-buffer
  presentation, multiple outputs and sessions, and a broader client-render
  protocol
- add richer display mode management and eventual GPU-accelerated composition
  without collapsing graphics policy into the desktop shell
- add support for multiple physical input hosts and broader pointer/button
  routing beyond the current single-host desktop path
- grow the desktop shell beyond current shortcuts, notifications, and task
  switching into notification history, gesture/snap/tiling/animation policy,
  file-opening/open-with flows, permissions UX, and richer system-status
  surfaces
- add broader graphical application and toolkit/runtime layers on top of the
  current app and window foundations
- add multiple shell and operator sessions, login/session ownership policy,
  package-installed command discovery, job control/pipelines, richer process
  environments, and richer operator history/status views
- grow the graphical terminal beyond current tabs, selection/copy-paste, and
  ANSI subset into split panes, fuller ANSI/VT coverage, richer clipboard
  integration, themes/profiles, better PTY resize semantics, and remote
  terminal/SSH workflows
- add graphical console surfaces and operator-session handoff/routing on top of
  the current serial console model

### Audio and media

- grow the current audio surface beyond tone playback into PCM/shared-buffer
  output, capture streams, mixing, per-app volume/session policy, and
  notification/media controls
- add codecs, containers, richer media pipelines, DMA-safe memory-object
  policy, and broader hardware backends beyond the current QEMU PC speaker path

### Developer workflows and observability

- grow the current developer tooling foundation beyond packaged sample
  workspaces into broader SDK/toolchain distribution, richer language
  ecosystems, runtime-aware build/run workflows, and stronger build-worker
  sandboxing
- add remote build farms and remote macOS build/sign/notarization integration
  on top of the current honest remote-only target model
- add IDE/editor integration and desktop-facing developer workflow UX without
  bypassing the shared shell/runtime path
- add persistent log storage, streaming log subscriptions, richer structured
  payload schemas, and better kernel trap ingestion into the log pipeline
- add richer service health reporting, subscription-based status monitoring,
  and shell/session status views
- add richer root-manager supervision and health policy without moving service
  lifecycle policy back into the kernel
