# Roadmap

## 1. Kernel Foundation

| Status | Area | Work Item |
|---|---|---|
| [x] | Kernel foundation | Firmware handoff, memory discovery, paging, and kernel heap bootstrap |
| [x] | Kernel foundation | Reusable kernel heap allocator with free-list reclamation |
| [x] | Kernel foundation | Interrupt, exception, syscall, and timer foundations |
| [x] | Kernel foundation | Kernel object model, capabilities, and IPC |
| [x] | Kernel foundation | Scheduler, task model, and userspace entry |
| [x] | Kernel foundation | Hardening, tests, and architecture cleanup |
| [ ] | Scheduling / memory / faults | Complete real CPU register context switching between unrelated kernel threads instead of the current bootstrap-oriented path |
| [ ] | Scheduling / memory / faults | Switch from the current boot-derived paging path to fully kernel-owned page tables and a fuller physical direct map |
| [ ] | Scheduling / memory / faults | Expose richer VM and memory-mapping syscalls needed for stronger runtimes, DMA-safe engines, and future process models |
| [ ] | Scheduling / memory / faults | Generalize memory-object mapping and shared-memory IPC beyond the current graphics-oriented use, and add richer object inspection and wait primitives |
| [ ] | Scheduling / memory / faults | Add fast `SYSCALL/SYSRET` on x86_64 alongside the current interrupt-gate syscall path |
| [ ] | Scheduling / memory / faults | Add preemptive time-slice enforcement, CPU-local run queues, and SMP scheduling and interrupt routing |
| [ ] | Scheduling / memory / faults | Add alternate x86 timer and interrupt sources such as LAPIC and HPET where they improve wake behavior and scheduling fidelity |
| [ ] | Scheduling / memory / faults | Extend the current fault-state propagation, manager supervision, and desktop fault surfacing into richer user-fault upcalls and recovery policy |

## 2. Root Bootstrap and Core Service Management

| Status | Area | Work Item |
|---|---|---|
| [x] | Root bootstrap | Root userspace service manager |
| [x] | Root bootstrap | Persisted boot-store manifest index |
| [x] | Root bootstrap | Dependency-ordered startup |
| [x] | Root bootstrap | Capability-scoped startup grants |
| [x] | Root bootstrap | Manager-mediated registration and discovery |
| [ ] | Packages / services | Add dynamic service installation, on-demand activation policy, and richer health-check definitions in manifests and package flows |
| [ ] | Observability / supervision | Add richer root-manager supervision and health policy without moving service lifecycle policy back into the kernel |
| [ ] | Bootstrap / service graph | Add smoother boot-mode selection, degraded-mode startup, and partial-graph recovery for failed service sets |
| [ ] | Bootstrap / orchestration | Add richer dependency diagnostics, cycle explanation, and startup timing/ordering visibility for service bring-up |

## 3. Foundational Services

| Status | Area | Work Item |
|---|---|---|
| [x] | Services | `storage-service` |
| [x] | Services | `console-service` |
| [x] | Services | `config-service` |
| [x] | Services | `log-service` |
| [x] | Services | `status-service` |
| [x] | Services | `network-service` |
| [x] | Services | Explicit lookup policy and per-handle transfer-right control |
| [ ] | Services / policy | Add richer service capability templates, revocation flows, and scoped delegation patterns across manager-launched services |
| [ ] | Services / status | Add richer structured health/status surfaces across all foundational services instead of ad hoc per-service reporting |
| [ ] | Services / resilience | Add service restart backoff, crash-loop detection, degraded-service state, and manager-facing escalation policy |

## 4. Storage, Filesystem, Packages, and Configuration

| Status | Area | Work Item |
|---|---|---|
| [ ] | Storage / filesystem | Replace the current in-memory writable overlay with persistent writable backing |
| [ ] | Storage / filesystem | Add writable storage and directory capabilities as first-class scoped authorities instead of broad ambient write access |
| [ ] | Storage / filesystem | Add block-device service contracts, mount management, namespace composition, and broader application-facing file and directory protocols |
| [ ] | Storage / policy | Add broader user-home and storage policy, plus writable project/workspace directories and persistent build outputs |
| [ ] | Configuration | Add namespaced service configuration trees, write and update policy, and schema validation and migration |
| [ ] | Storage / indexing | Add file indexing, metadata queries, search primitives, and content discovery support for desktop and developer workflows |
| [ ] | Storage / sharing | Add explicit file-sharing, open-with, recent-files, and app-association policy on top of the core file and directory protocols |

## 5. Platform Tooling and Package Foundations

| Status | Area | Work Item |
|---|---|---|
| [x] | Tooling | `shell-service` operator environment |
| [x] | Tooling | `package-service` install, update, rollback, and activation coordination |
| [ ] | Packages | Add network-backed repositories, signed feeds, trust metadata, writable install roots, install journals, and rollback policy for packages |
| [ ] | Packages / updates | Add whole-system image update workflows on top of the package/update foundation |
| [ ] | Packages / trust | Add package trust state, source provenance, signing status, and rollback provenance inspection through the package-service contract |
| [ ] | Packages / policy | Add package pinning, channels/rings, staged rollouts, and policy-controlled upgrade rules |
| [ ] | Packages / recovery | Add better interrupted-install recovery, consistency checking, garbage collection, and repair flows for partially applied updates |

## 6. Architecture and Platform Abstraction

| Status | Area | Work Item |
|---|---|---|
| [x] | Architecture | Explicit `kernel/core`, `arch/<isa>`, and `platform/<platform>` layering |
| [x] | Architecture | `arch/x86_64` as the active ISA crate |
| [x] | Architecture | `platform/x86_64/qemu_virtio` for UEFI, serial, framebuffer, input, and VirtIO backend wiring |
| [x] | Architecture | `arch/aarch64` and `platform/aarch64/raspi5` as first-class targets with a native Raspberry Pi 5 image and serial-first userspace bootstrap |
| [x] | Architecture | Platform-first `xtask` build, image, and run selection |
| [x] | Architecture | Normalized `BootInfo` handoff into generic kernel initialization |
| [ ] | Platform / hardware follow-on | Add Raspberry Pi 5 framebuffer, graphical input, networking, writable boot-store, and audio backends behind the current `platform/aarch64/raspi5` contracts |
| [ ] | Platform / hardware follow-on | Add Wi-Fi and Bluetooth controller backends behind the relevant `platform/<platform>` contracts as supported hardware targets expand |
| [ ] | Platform / hardware follow-on | Expand Raspberry Pi 5 beyond the current serial-first bootstrap into the normal graphical and network-backed service graph |
| [ ] | Platform / targets | Add `platform/aarch64/virt` or equivalent clean ARM virtual target to keep ARM bring-up separate from Pi-specific board quirks |
| [ ] | Platform / targets | Add a second x86 platform target and split the remaining x86 PC interrupt-controller details out of `arch/x86_64` if needed |
| [ ] | Platform / targets | Add additional future-facing targets such as real x86 PC hardware and/or `riscv64/virt` once the current platform layering stabilizes |

## 7. Networking

| Status | Area | Work Item |
|---|---|---|
| [ ] | Networking | Grow the current networking surface beyond DHCP/DNS/ICMP/outbound TCP into UDP, inbound/listening transports, richer routing, multi-interface policy, and IPv6 |
| [ ] | Networking | Add long-lived resolver caching, richer DNS record support, firewalling, and broader network policy |
| [ ] | Networking / wireless | Add Wi-Fi device support, network scanning, join/auth flows, saved networks, and wireless configuration behind the current network-service contract |
| [ ] | Networking / wireless | Add richer wireless policy including roaming, security modes, per-network trust/configuration, and desktop-facing wireless state surfaces |
| [ ] | Networking / performance | Move beyond copied-frame transport into packet-buffer sharing and zero-copy networking |
| [ ] | Networking / drivers | Add richer NIC interrupt models such as MSI/MSI-X, additional virtual backends, and real NIC driver hosts beyond current QEMU/VirtIO |
| [ ] | Networking / services | Add richer local service discovery, host naming, and network diagnostics tooling for desktop and developer workflows |

## 8. Graphics and Session Foundation

| Status | Area | Work Item |
|---|---|---|
| [x] | Graphics / session | Kernel display-output object |
| [x] | Graphics / session | `graphics-service` output, surface, and composition ownership |
| [x] | Graphics / session | `session-service` focus and graphical session ownership |
| [x] | Graphics / session | Shell/operator inspection of outputs, surfaces, and sessions |
| [ ] | Graphics | Grow the current mapped-buffer graphics path into damage-tracked multi-buffer presentation, multiple outputs and sessions, and a broader client-render protocol |
| [ ] | Graphics | Grow the current shared-buffer graphics path into mapped or zero-copy presentation buffers and a broader client-render protocol for richer clients |
| [ ] | Graphics | Add richer display mode management and eventual GPU-accelerated composition without collapsing graphics policy into the desktop shell |
| [ ] | Input | Move input delivery from PIT polling to device-driven wakeups, mirroring the packet-interface path |
| [ ] | Input | Add support for multiple physical input hosts and broader pointer/button routing beyond the current single-host desktop path |
| [ ] | Session / composition | Add richer session switching, seat ownership, session handoff, and isolation policy across multiple graphical and operator sessions |

## 9. Desktop Shell and Interaction

| Status | Area | Work Item |
|---|---|---|
| [x] | Desktop shell | `desktop-shell-service` graphical shell chrome and launcher surface ownership |
| [x] | Desktop shell | Manager-mediated graphical app launch and focus handling |
| [x] | Desktop shell | First core app set: `settings-app`, `files-app`, `monitor-app` |
| [x] | Desktop shell | Serial-shell inspection of desktop state through the desktop-shell contract |
| [x] | Desktop interaction | Window focus, z-order, move, resize, minimize, restore, and close handling in `desktop-shell-service` |
| [x] | Desktop interaction | Maximize handling in `desktop-shell-service` |
| [x] | Desktop interaction | App-control channel for desktop-driven focus, resize, close, pointer, key, and text events |
| [x] | Desktop interaction | Physical input routing from the kernel input-source object through `session-service` into the desktop interaction contract |
| [x] | Desktop interaction | Shell/operator inspection and control of desktop windows through the desktop-shell contract |
| [ ] | Desktop shell | Grow the desktop shell beyond current shortcuts, notifications, and task switching into notification history, gesture/snap/tiling/animation policy, file-opening/open-with flows, permissions UX, and richer system-status surfaces |
| [ ] | Desktop shell | Add richer task switching, broader shortcut policy, gestures, smoother windowing and animation behavior, and a more macOS-like interaction feel without collapsing shell logic into platform services |
| [ ] | Desktop shell | Add richer desktop search, launcher ranking, app switching history, workspaces/spaces, and multi-monitor shell behavior |
| [ ] | Desktop shell | Add desktop-level clipboard, drag-and-drop, file handoff, and app-intent/open-with flows across apps and services |
| [ ] | Desktop shell | Add permissions-aware desktop UX surfaces for app authority, notifications, file access, device access, and runtime launches |
| [ ] | Desktop shell / wireless | Add desktop Wi-Fi UX for scanning, joining, saved networks, signal state, and connection troubleshooting |
| [ ] | Desktop shell / Bluetooth | Add desktop Bluetooth UX for pairing, device management, battery/state display, and audio/input routing |
| [ ] | Apps / UI | Add broader graphical application and toolkit/runtime layers on top of the current app and window foundations |

## 10. Terminal, Shell, Console, and Operator UX

| Status | Area | Work Item |
|---|---|---|
| [ ] | Shell / sessions | Add multiple shell and operator sessions, login/session ownership policy, package-installed command discovery, job control/pipelines, richer process environments, and richer operator history/status views |
| [ ] | Terminal | Grow the graphical terminal beyond current tabs, selection/copy-paste, and ANSI subset into split panes, fuller ANSI/VT coverage, richer clipboard integration, themes/profiles, better PTY resize semantics, and remote terminal/SSH workflows |
| [ ] | Terminal | Add terminal session persistence/reattach, command bookmarking, session restore, and richer command/result inspection |
| [ ] | Console | Add graphical console surfaces and operator-session handoff/routing on top of the current serial console model |
| [ ] | Operator UX | Add richer shell/operator diagnostics, log-following, structured status views, and app/runtime inspection tooling without bypassing service contracts |

## 11. Execution, Loading, Compatibility, and Runtimes

| Status | Area | Work Item |
|---|---|---|
| [x] | Runtime foundation | Package-delivered `runtime-service` for explicit compatibility/runtime environments |
| [x] | Runtime foundation | Environment creation, inspection, run launch, and teardown through a real userspace service contract |
| [x] | Runtime foundation | Explicit mount and variable mapping for the first `posix` runtime profile |
| [x] | Runtime foundation | Manager-mediated launch of runtime-hosted workloads instead of shell-owned compatibility shortcuts |
| [x] | Runtime foundation | Shared shell/terminal operator integration for runtime inspection and launch |
| [ ] | Loading / execution | Grow the current stored flat-image loader into ELF and other richer executable formats, dependency loading, and broader runtime policy |
| [ ] | Loading / execution | Add a general process loader for user-supplied images instead of only manager-owned stored images |
| [ ] | Compatibility | Grow the current compatibility/runtime foundation beyond hosted `posix` environments into Linux-oriented ABI expansion, arbitrary ELF execution, richer runtime packaging, and desktop launch UX for runtime-hosted apps |
| [ ] | Compatibility / security | Add explicit capability grants for network, graphics, input, and audio to compatibility workloads |
| [ ] | Compatibility | Add Windows runtime support and broader cross-platform application execution |
| [ ] | Compatibility / security | Add stronger sandboxing and container-style isolation for compatibility workloads |
| [ ] | Runtime UX | Add desktop-facing runtime launch surfaces, runtime state inspection, and app/runtime distinction UX for native versus hosted applications |

## 12. Audio and Media

| Status | Area | Work Item |
|---|---|---|
| [ ] | Audio | Grow the current audio surface beyond tone playback into PCM/shared-buffer output, capture streams, mixing, per-app volume/session policy, and notification/media controls |
| [ ] | Audio / hardware | Add broader hardware audio backends beyond the current wired/emulated paths, including Bluetooth audio and future USB audio paths |
| [ ] | Audio / Bluetooth | Add Bluetooth audio output/input support, pairing integration, and endpoint routing on top of the broader audio platform |
| [ ] | Media | Add codecs, containers, richer media pipelines, DMA-safe memory-object policy, and broader hardware backends beyond the current QEMU PC speaker path |
| [ ] | Audio / UX | Add desktop-facing volume, endpoint selection, per-app audio policy, and notification/media playback integration |
| [ ] | Media / apps | Add first-party media playback/preview apps and richer integration with files, notifications, and runtime-hosted apps |

## 13. Developer Tooling and Workflows

| Status | Area | Work Item |
|---|---|---|
| [x] | Developer tooling | Package-delivered `developer-service` for toolchain, workspace, build-job, and artifact management |
| [x] | Developer tooling | Packaged toolchain descriptors for native, Linux, Windows, and honest remote-only macOS target metadata |
| [x] | Developer tooling | Packaged workspace descriptors and sample source payloads for the first cross-target workflow |
| [x] | Developer tooling | Manager-mediated launch of transient `cross-builder-tool` workers instead of shell-owned build shortcuts |
| [x] | Developer tooling | Shared shell/terminal operator integration for toolchain inspection, build submission, job inspection, and artifact export |
| [ ] | Developer workflows | Grow the current developer tooling foundation beyond packaged sample workspaces into broader SDK/toolchain distribution, richer language ecosystems, runtime-aware build/run workflows, and stronger build-worker sandboxing |
| [ ] | Developer workflows / infra | Add remote build farms and remote macOS build/sign/notarization integration on top of the current honest remote-only target model |
| [ ] | Developer UX | Add IDE/editor integration and desktop-facing developer workflow UX without bypassing the shared shell/runtime path |
| [ ] | Developer UX | Add project browser, artifact viewer, debugger/profiler hooks, and richer build/test/run surfaces in the desktop environment |
| [ ] | Developer trust / policy | Add explicit developer-tool permissions, workspace trust policy, and project/runtime authority review surfaces |

## 14. Permissions, Trust, and Security UX

| Status | Area | Work Item |
|---|---|---|
| [ ] | Permissions | Add clearer app permissions and capability review surfaces for native apps, runtime-hosted apps, and developer tools |
| [ ] | Runtime permissions | Add runtime permission prompts/policy for hosted environments and compatibility workloads without replacing real capability enforcement with fake prompts |
| [ ] | Trust / signing UX | Add trust/signing UX for packages, runtimes, repositories, and developer artifacts with honest visibility into what is actually enforced |
| [ ] | Package trust UI | Add package trust UI showing provenance, signing state, update trust, and rollback provenance in both terminal and desktop flows |
| [ ] | Desktop security surfaces | Add desktop security surfaces for privileged actions, permission review, trust warnings, and authority escalation flows |
| [ ] | Security UX | Add consistent denial/error UX and operator diagnostics when capability checks, package trust checks, or runtime policy checks fail |
| [ ] | Security policy | Add stronger permission editing/review flows, revocation UX, and capability grant history without moving policy into the shell or UI code |
| [ ] | Security foundations | Add stronger signing/key management, trust roots, sandbox policy expansion, and audit support under the current permission and trust UX surfaces |

## 15. Software Distribution and App Ecosystem

| Status | Area | Work Item |
|---|---|---|
| [ ] | Software center | Add a software center / app-store style GUI on top of the package/update foundation |
| [ ] | Installer / updater UX | Add better installer/update UX, including source selection, trust display, progress, rollback visibility, and interrupted-update recovery |
| [ ] | App lifecycle | Add clean third-party app lifecycle flows for install, launch, update, remove, permissions review, and file/open-with association |
| [ ] | App ecosystem | Add app metadata/index/search, screenshots/descriptions, categories, recommendations, and package/runtime compatibility surfacing |
| [ ] | App distribution | Add third-party repository onboarding, trust review, side-loading policy, and package/runtime compatibility handling |
| [ ] | App policy | Add per-app default associations, intent/open-with policy, recent apps/documents, and uninstall cleanup behavior |

## 16. Desktop Polish and Advanced Interaction

| Status | Area | Work Item |
|---|---|---|
| [ ] | Desktop polish | Add richer task switching UX, smoother animations, gestures, and more macOS-like desktop feel on top of the existing shell and interaction model |
| [ ] | Desktop polish | Add richer notifications, notification history, quick actions, and permissions-aware desktop surfaces |
| [ ] | Desktop polish | Add smoother windowing behavior, animation timing, shadowing, transitions, and interaction polish without collapsing shell logic into compositor policy |
| [ ] | Desktop polish | Add richer shortcut systems, command palette/search UX, global action routing, and cross-app interaction patterns |
| [ ] | Desktop polish | Add broader drag-and-drop, clipboard history, desktop gestures, hot corners, and richer shell affordances |
| [ ] | Desktop polish | Add accessibility surfaces such as keyboard-first navigation, high-contrast/visual accessibility settings, zoom, and assistive interaction hooks |

## 17. Observability, Logging, and Status

| Status | Area | Work Item |
|---|---|---|
| [ ] | Logging / observability | Add persistent log storage, streaming log subscriptions, richer structured payload schemas, and better kernel trap ingestion into the log pipeline |
| [ ] | Status / observability | Add richer service health reporting, subscription-based status monitoring, and shell/session status views |
| [ ] | Observability / desktop | Add desktop-facing logs, crash reports, service health surfaces, and app/runtime diagnostics on top of the current logging/status foundations |
| [ ] | Observability / developer | Add richer trace capture, performance/event timeline views, and operator/developer diagnostics for graphics, networking, runtimes, and builds |

## 18. Toward a More Complete OS

| Status | Area | Work Item |
|---|---|---|
| [ ] | Accounts / identity | Add user accounts, login/session ownership, identity switching, and user-scoped policy across storage, apps, runtimes, and services |
| [ ] | Backup / restore | Add backup, restore, migration, and state export/import flows for user data, apps, packages, and system configuration |
| [ ] | Printing / peripherals | Add printer/peripheral service contracts and desktop-facing peripheral management as hardware support broadens |
| [ ] | Peripherals / wireless | Add broader peripheral connectivity including Bluetooth input devices, wireless accessories, and consumer-device management flows |
| [ ] | Power / devices | Add suspend/resume, power policy, battery/thermal/device health reporting, and laptop-oriented desktop/system behavior |
| [ ] | Installation / onboarding | Add a real installer, setup/onboarding flows, recovery environment, and first-boot configuration experience |
| [ ] | Hardening / release | Add release engineering, reproducible builds, artifact signing, installer images, upgrade test matrices, and broader end-to-end validation for a near-complete OS |
