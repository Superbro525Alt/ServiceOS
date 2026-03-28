# Roadmap

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

## Next

- add Raspberry Pi 5 framebuffer, input, networking, and writable-boot-store
  backends behind the current `platform/aarch64/raspi5` contracts
- evolve the flat-image bootstrap into a richer executable-loading model when
  the current userspace service graph outgrows it
- add writable storage and directory capabilities
- extend terminate-on-fault isolation into richer user-fault upcalls and
  recovery policy
- move input delivery from PIT polling to device-driven wakeups, mirroring the
  packet-interface path
- grow the current networking surface beyond DHCP/DNS/ICMP/outbound TCP into
  UDP, inbound/listening transports, richer routing, and IPv6
- grow the current audio surface beyond tone playback into PCM output, capture
  streams, mixing, per-app policy, and broader hardware backends
- split the remaining x86 PC interrupt-controller details out of `arch/x86_64`
  if a second x86 platform target is added
- grow the current shared-buffer graphics path into mapped or zero-copy
  presentation buffers and a broader client-render protocol
- grow the first desktop shell into richer task switching, broader shortcut and
  gesture policy, notifications, and permissions-aware desktop UX without
  collapsing it into platform services
- grow the first graphical terminal into tabs, selection/copy-paste, richer
  ANSI/VT handling, and better resize semantics on top of the current
  `terminal-service` boundary

## Later

- richer storage/filesystem services
- broader graphical application and toolkit ecosystem
- compatibility runtimes
