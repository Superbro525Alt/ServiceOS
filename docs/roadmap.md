# Roadmap

## 1. Kernel Foundation

| Status | Area | Work Item |
|---|---|---|
| &#x2705; | Kernel foundation | Firmware handoff, memory discovery, paging, and kernel heap bootstrap |
| &#x2705; | Kernel foundation | Reusable kernel heap allocator with free-list reclamation |
| &#x2705; | Kernel foundation | Interrupt, exception, syscall, and timer foundations |
| &#x2705; | Kernel foundation | Kernel object model, capabilities, and IPC |
| &#x2705; | Kernel foundation | Scheduler, task model, and userspace entry |
| &#x2705; | Kernel foundation | Hardening, tests, and architecture cleanup |
| &#x2B1C; | Scheduling / memory / faults | Complete real CPU register context switching between unrelated kernel threads instead of the current bootstrap-oriented path |
| &#x2B1C; | Scheduling / memory / faults | Switch from the current boot-derived paging path to fully kernel-owned page tables and a fuller physical direct map |
| &#x2B1C; | Scheduling / memory / faults | Expose richer VM and memory-mapping syscalls needed for stronger runtimes, DMA-safe engines, and future process models |
| &#x2B1C; | Scheduling / memory / faults | Generalize memory-object mapping and shared-memory IPC beyond the current graphics-oriented use, and add richer object inspection and wait primitives |
| &#x2B1C; | Scheduling / memory / faults | Add fast `SYSCALL/SYSRET` on x86_64 alongside the current interrupt-gate syscall path |
| &#x2B1C; | Scheduling / memory / faults | Add preemptive time-slice enforcement, CPU-local run queues, and SMP scheduling and interrupt routing |
| &#x2B1C; | Scheduling / memory / faults | Add alternate x86 timer and interrupt sources such as LAPIC and HPET where they improve wake behavior and scheduling fidelity |
| &#x2B1C; | Scheduling / memory / faults | Extend the current fault-state propagation, manager supervision, and desktop fault surfacing into richer user-fault upcalls and recovery policy |

## 2. Root Bootstrap and Core Service Management

| Status | Area | Work Item |
|---|---|---|
| &#x2705; | Root bootstrap | Root userspace service manager |
| &#x2705; | Root bootstrap | Persisted boot-store manifest index |
| &#x2705; | Root bootstrap | Dependency-ordered startup |
| &#x2705; | Root bootstrap | Capability-scoped startup grants |
| &#x2705; | Root bootstrap | Manager-mediated registration and discovery |
| &#x2B1C; | Packages / services | Add dynamic service installation, on-demand activation policy, and richer health-check definitions in manifests and package flows |
| &#x2B1C; | Observability / supervision | Add richer root-manager supervision and health policy without moving service lifecycle policy back into the kernel |
| &#x2B1C; | Bootstrap / service graph | Add smoother boot-mode selection, degraded-mode startup, and partial-graph recovery for failed service sets |
| &#x2B1C; | Bootstrap / orchestration | Add richer dependency diagnostics, cycle explanation, and startup timing/ordering visibility for service bring-up |

## 3. Foundational Services

| Status | Area | Work Item |
|---|---|---|
| &#x2705; | Services | `storage-service` |
| &#x2705; | Services | `console-service` |
| &#x2705; | Services | `config-service` |
| &#x2705; | Services | `log-service` |
| &#x2705; | Services | `status-service` |
| &#x2705; | Services | `network-service` |
| &#x2705; | Services | Explicit lookup policy and per-handle transfer-right control |
| &#x2B1C; | Services / policy | Add richer service capability templates, revocation flows, and scoped delegation patterns across manager-launched services |
| &#x2B1C; | Services / status | Add richer structured health/status surfaces across all foundational services instead of ad hoc per-service reporting |
| &#x2B1C; | Services / resilience | Add service restart backoff, crash-loop detection, degraded-service state, and manager-facing escalation policy |

## 4. Storage, Filesystem, Packages, and Configuration

| Status | Area | Work Item |
|---|---|---|
| &#x2B1C; | Storage / filesystem | Replace the current in-memory writable overlay with persistent writable backing |
| &#x2705; | Storage / filesystem | Add writable storage and directory capabilities as first-class scoped authorities instead of broad ambient write access |
| &#x2B1C; | Storage / filesystem | Add block-device service contracts, mount management, namespace composition, and broader application-facing file and directory protocols |
| &#x2B1C; | Storage / policy | Add broader user-home and storage policy, plus writable project/workspace directories and persistent build outputs |
| &#x2B1C; | Configuration | Add namespaced service configuration trees, write and update policy, and schema validation and migration |
| &#x2B1C; | Storage / indexing | Add file indexing, metadata queries, search primitives, and content discovery support for desktop and developer workflows |
| &#x2B1C; | Storage / sharing | Add explicit file-sharing, open-with, recent-files, and app-association policy on top of the core file and directory protocols |

## 5. Platform Tooling and Package Foundations

| Status | Area | Work Item |
|---|---|---|
| &#x2705; | Tooling | `shell-service` operator environment |
| &#x2705; | Tooling | `package-service` install, update, rollback, and activation coordination |
| &#x2B1C; | Packages | Add network-backed repositories, signed feeds, trust metadata, writable install roots, install journals, and rollback policy for packages |
| &#x2B1C; | Packages / updates | Add whole-system image update workflows on top of the package/update foundation |
| &#x2B1C; | Packages / trust | Add package trust state, source provenance, signing status, and rollback provenance inspection through the package-service contract |
| &#x2B1C; | Packages / policy | Add package pinning, channels/rings, staged rollouts, and policy-controlled upgrade rules |
| &#x2B1C; | Packages / recovery | Add better interrupted-install recovery, consistency checking, garbage collection, and repair flows for partially applied updates |

## 6. Architecture and Platform Abstraction

| Status | Area | Work Item |
|---|---|---|
| &#x2705; | Architecture | Explicit `kernel/core`, `arch/<isa>`, and `platform/<platform>` layering |
| &#x2705; | Architecture | `arch/x86_64` as the active ISA crate |
| &#x2705; | Architecture | `platform/x86_64/qemu_virtio` for UEFI, serial, framebuffer, input, and VirtIO backend wiring |
| &#x2705; | Architecture | `arch/aarch64` and `platform/aarch64/raspi5` as first-class targets with a native Raspberry Pi 5 image and serial-first userspace bootstrap |
| &#x2705; | Architecture | Platform-first `xtask` build, image, and run selection |
| &#x2705; | Architecture | Normalized `BootInfo` handoff into generic kernel initialization |
| &#x2B1C; | Platform / hardware follow-on | Add Raspberry Pi 5 framebuffer, graphical input, networking, writable boot-store, and audio backends behind the current `platform/aarch64/raspi5` contracts |
| &#x2B1C; | Platform / hardware follow-on | Add Wi-Fi and Bluetooth controller backends behind the relevant `platform/<platform>` contracts as supported hardware targets expand |
| &#x2B1C; | Platform / hardware follow-on | Expand Raspberry Pi 5 beyond the current serial-first bootstrap into the normal graphical and network-backed service graph |
| &#x2B1C; | Platform / targets | Add `platform/aarch64/virt` or equivalent clean ARM virtual target to keep ARM bring-up separate from Pi-specific board quirks |
| &#x2B1C; | Platform / targets | Add a second x86 platform target and split the remaining x86 PC interrupt-controller details out of `arch/x86_64` if needed |
| &#x2B1C; | Platform / targets | Add additional future-facing targets such as real x86 PC hardware and/or `riscv64/virt` once the current platform layering stabilizes |

## 7. Networking

| Status | Area | Work Item |
|---|---|---|
| &#x2B1C; | Networking | Grow the current networking surface beyond DHCP/DNS/ICMP/outbound TCP into UDP, inbound/listening transports, richer routing, multi-interface policy, and IPv6 |
| &#x2B1C; | Networking | Add long-lived resolver caching, richer DNS record support, firewalling, and broader network policy |
| &#x2B1C; | Networking / wireless | Add Wi-Fi device support, network scanning, join/auth flows, saved networks, and wireless configuration behind the current network-service contract |
| &#x2B1C; | Networking / wireless | Add richer wireless policy including roaming, security modes, per-network trust/configuration, and desktop-facing wireless state surfaces |
| &#x2B1C; | Networking / performance | Move beyond copied-frame transport into packet-buffer sharing and zero-copy networking |
| &#x2B1C; | Networking / drivers | Add richer NIC interrupt models such as MSI/MSI-X, additional virtual backends, and real NIC driver hosts beyond current QEMU/VirtIO |
| &#x2B1C; | Networking / services | Add richer local service discovery, host naming, and network diagnostics tooling for desktop and developer workflows |

## 8. Graphics and Session Foundation

| Status | Area | Work Item |
|---|---|---|
| &#x2705; | Graphics / session | Kernel display-output object |
| &#x2705; | Graphics / session | `graphics-service` output, surface, and composition ownership |
| &#x2705; | Graphics / session | `session-service` focus and graphical session ownership |
| &#x2705; | Graphics / session | Shell/operator inspection of outputs, surfaces, and sessions |
| &#x2B1C; | Graphics | Grow the current mapped-buffer graphics path into damage-tracked multi-buffer presentation, multiple outputs and sessions, and a broader client-render protocol |
| &#x2B1C; | Graphics | Grow the current shared-buffer graphics path into mapped or zero-copy presentation buffers and a broader client-render protocol for richer clients |
| &#x2B1C; | Graphics | Add richer display mode management and eventual GPU-accelerated composition without collapsing graphics policy into the desktop shell |
| &#x2705; | Input | Move active input delivery from polling to device-driven wakeups through the platform IRQ path, while keeping a blocking receive-side fallback for missed edges |
| &#x2B1C; | Input | Add support for multiple physical input hosts and broader pointer/button routing beyond the current single-host desktop path |
| &#x2B1C; | Session / composition | Add richer session switching, seat ownership, session handoff, and isolation policy across multiple graphical and operator sessions |

## 9. Desktop Shell and Interaction

| Status | Area | Work Item |
|---|---|---|
| &#x2705; | Desktop shell | `desktop-shell-service` graphical shell chrome and launcher surface ownership |
| &#x2705; | Desktop shell | Manager-mediated graphical app launch and focus handling |
| &#x2705; | Desktop shell | First core app set: `settings-app`, `files-app`, `monitor-app` |
| &#x2705; | Desktop shell | Serial-shell inspection of desktop state through the desktop-shell contract |
| &#x2705; | Desktop interaction | Window focus, z-order, move, resize, minimize, restore, and close handling in `desktop-shell-service` |
| &#x2705; | Desktop interaction | Maximize handling in `desktop-shell-service` |
| &#x2705; | Desktop interaction | App-control channel for desktop-driven focus, resize, close, pointer, key, and text events |
| &#x2705; | Desktop interaction | Physical input routing from the kernel input-source object through `session-service` into the desktop interaction contract |
| &#x2705; | Desktop interaction | Shell/operator inspection and control of desktop windows through the desktop-shell contract |
| &#x2B1C; | Desktop shell | Grow the desktop shell beyond current shortcuts, notifications, and task switching into notification history, gesture/snap/tiling/animation policy, file-opening/open-with flows, permissions UX, and richer system-status surfaces |
| &#x2B1C; | Desktop shell | Add richer task switching, broader shortcut policy, gestures, smoother windowing and animation behavior, and a more macOS-like interaction feel without collapsing shell logic into platform services |
| &#x2B1C; | Desktop shell | Add richer desktop search, launcher ranking, app switching history, workspaces/spaces, and multi-monitor shell behavior |
| &#x2B1C; | Desktop shell | Add desktop-level clipboard, drag-and-drop, file handoff, and app-intent/open-with flows across apps and services |
| &#x2B1C; | Desktop shell | Add permissions-aware desktop UX surfaces for app authority, notifications, file access, device access, and runtime launches |
| &#x2B1C; | Desktop shell / wireless | Add desktop Wi-Fi UX for scanning, joining, saved networks, signal state, and connection troubleshooting |
| &#x2B1C; | Desktop shell / Bluetooth | Add desktop Bluetooth UX for pairing, device management, battery/state display, and audio/input routing |
| &#x2B1C; | Apps / UI | Add broader graphical application and toolkit/runtime layers on top of the current app and window foundations |

## 10. Terminal, Shell, Console, and Operator UX

| Status | Area | Work Item |
|---|---|---|
| &#x2B1C; | Shell / sessions | Add multiple shell and operator sessions, login/session ownership policy, package-installed command discovery, job control/pipelines, richer process environments, and richer operator history/status views |
| &#x2B1C; | Terminal | Grow the graphical terminal beyond current tabs, selection/copy-paste, and ANSI subset into split panes, fuller ANSI/VT coverage, richer clipboard integration, themes/profiles, better PTY resize semantics, and remote terminal/SSH workflows |
| &#x2B1C; | Terminal | Add terminal session persistence/reattach, command bookmarking, session restore, and richer command/result inspection |
| &#x2B1C; | Console | Add graphical console surfaces and operator-session handoff/routing on top of the current serial console model |
| &#x2B1C; | Operator UX | Add richer shell/operator diagnostics, log-following, structured status views, and app/runtime inspection tooling without bypassing service contracts |

## 11. Execution, Loading, Compatibility, and Runtimes

| Status | Area | Work Item |
|---|---|---|
| &#x2705; | Runtime foundation | Package-delivered `runtime-service` for explicit compatibility/runtime environments |
| &#x2705; | Runtime foundation | Environment creation, inspection, run launch, and teardown through a real userspace service contract |
| &#x2705; | Runtime foundation | Explicit mount and variable mapping for the first `posix` runtime profile |
| &#x2705; | Runtime foundation | Manager-mediated launch of runtime-hosted workloads instead of shell-owned compatibility shortcuts |
| &#x2705; | Runtime foundation | Shared shell/terminal operator integration for runtime inspection and launch |
| &#x2B1C; | Loading / execution | Grow the current stored flat-image loader into ELF and other richer executable formats, dependency loading, and broader runtime policy |
| &#x2B1C; | Loading / execution | Add a general process loader for user-supplied images instead of only manager-owned stored images |
| &#x2B1C; | Compatibility | Grow the current compatibility/runtime foundation beyond hosted `posix` environments into Linux-oriented ABI expansion, arbitrary ELF execution, richer runtime packaging, and desktop launch UX for runtime-hosted apps |
| &#x2B1C; | Compatibility / security | Add explicit capability grants for network, graphics, input, and audio to compatibility workloads |
| &#x2B1C; | Compatibility | Add Windows runtime support and broader cross-platform application execution |
| &#x2B1C; | Compatibility / security | Add stronger sandboxing and container-style isolation for compatibility workloads |
| &#x2B1C; | Runtime UX | Add desktop-facing runtime launch surfaces, runtime state inspection, and app/runtime distinction UX for native versus hosted applications |

## 12. Audio and Media

| Status | Area | Work Item |
|---|---|---|
| &#x2B1C; | Audio | Grow the current audio surface beyond tone playback into PCM/shared-buffer output, capture streams, mixing, per-app volume/session policy, and notification/media controls |
| &#x2B1C; | Audio / hardware | Add broader hardware audio backends beyond the current wired/emulated paths, including Bluetooth audio and future USB audio paths |
| &#x2B1C; | Audio / Bluetooth | Add Bluetooth audio output/input support, pairing integration, and endpoint routing on top of the broader audio platform |
| &#x2B1C; | Media | Add codecs, containers, richer media pipelines, DMA-safe memory-object policy, and broader hardware backends beyond the current QEMU PC speaker path |
| &#x2B1C; | Audio / UX | Add desktop-facing volume, endpoint selection, per-app audio policy, and notification/media playback integration |
| &#x2B1C; | Media / apps | Add first-party media playback/preview apps and richer integration with files, notifications, and runtime-hosted apps |

## 13. Developer Tooling and Workflows

| Status | Area | Work Item |
|---|---|---|
| &#x2705; | Developer tooling | Package-delivered `developer-service` for toolchain, workspace, build-job, and artifact management |
| &#x2705; | Developer tooling | Packaged toolchain descriptors for native, Linux, Windows, and honest remote-only macOS target metadata |
| &#x2705; | Developer tooling | Packaged workspace descriptors and sample source payloads for the first cross-target workflow |
| &#x2705; | Developer tooling | Manager-mediated launch of transient `cross-builder-tool` workers instead of shell-owned build shortcuts |
| &#x2705; | Developer tooling | Shared shell/terminal operator integration for toolchain inspection, build submission, job inspection, and artifact export |
| &#x2B1C; | Developer workflows | Grow the current developer tooling foundation beyond packaged sample workspaces into broader SDK/toolchain distribution, richer language ecosystems, runtime-aware build/run workflows, and stronger build-worker sandboxing |
| &#x2B1C; | Developer workflows / infra | Add remote build farms and remote macOS build/sign/notarization integration on top of the current honest remote-only target model |
| &#x2B1C; | Developer UX | Add IDE/editor integration and desktop-facing developer workflow UX without bypassing the shared shell/runtime path |
| &#x2B1C; | Developer UX | Add project browser, artifact viewer, debugger/profiler hooks, and richer build/test/run surfaces in the desktop environment |
| &#x2B1C; | Developer trust / policy | Add explicit developer-tool permissions, workspace trust policy, and project/runtime authority review surfaces |

## 14. Permissions, Trust, and Security UX

| Status | Area | Work Item |
|---|---|---|
| &#x2B1C; | Permissions | Add clearer app permissions and capability review surfaces for native apps, runtime-hosted apps, and developer tools |
| &#x2B1C; | Runtime permissions | Add runtime permission prompts/policy for hosted environments and compatibility workloads without replacing real capability enforcement with fake prompts |
| &#x2B1C; | Trust / signing UX | Add trust/signing UX for packages, runtimes, repositories, and developer artifacts with honest visibility into what is actually enforced |
| &#x2B1C; | Package trust UI | Add package trust UI showing provenance, signing state, update trust, and rollback provenance in both terminal and desktop flows |
| &#x2B1C; | Desktop security surfaces | Add desktop security surfaces for privileged actions, permission review, trust warnings, and authority escalation flows |
| &#x2B1C; | Security UX | Add consistent denial/error UX and operator diagnostics when capability checks, package trust checks, or runtime policy checks fail |
| &#x2B1C; | Security policy | Add stronger permission editing/review flows, revocation UX, and capability grant history without moving policy into the shell or UI code |
| &#x2B1C; | Security foundations | Add stronger signing/key management, trust roots, sandbox policy expansion, and audit support under the current permission and trust UX surfaces |

## 15. Software Distribution and App Ecosystem

| Status | Area | Work Item |
|---|---|---|
| &#x2B1C; | Software center | Add a software center / app-store style GUI on top of the package/update foundation |
| &#x2B1C; | Installer / updater UX | Add better installer/update UX, including source selection, trust display, progress, rollback visibility, and interrupted-update recovery |
| &#x2B1C; | App lifecycle | Add clean third-party app lifecycle flows for install, launch, update, remove, permissions review, and file/open-with association |
| &#x2B1C; | App ecosystem | Add app metadata/index/search, screenshots/descriptions, categories, recommendations, and package/runtime compatibility surfacing |
| &#x2B1C; | App distribution | Add third-party repository onboarding, trust review, side-loading policy, and package/runtime compatibility handling |
| &#x2B1C; | App policy | Add per-app default associations, intent/open-with policy, recent apps/documents, and uninstall cleanup behavior |

## 16. Desktop Polish and Advanced Interaction

| Status | Area | Work Item |
|---|---|---|
| &#x2B1C; | Desktop polish | Add richer task switching UX, smoother animations, gestures, and more macOS-like desktop feel on top of the existing shell and interaction model |
| &#x2B1C; | Desktop polish | Add richer notifications, notification history, quick actions, and permissions-aware desktop surfaces |
| &#x2B1C; | Desktop polish | Add smoother windowing behavior, animation timing, shadowing, transitions, and interaction polish without collapsing shell logic into compositor policy |
| &#x2B1C; | Desktop polish | Add richer shortcut systems, command palette/search UX, global action routing, and cross-app interaction patterns |
| &#x2B1C; | Desktop polish | Add broader drag-and-drop, clipboard history, desktop gestures, hot corners, and richer shell affordances |
| &#x2B1C; | Desktop polish | Add accessibility surfaces such as keyboard-first navigation, high-contrast/visual accessibility settings, zoom, and assistive interaction hooks |

## 17. Observability, Logging, and Status

| Status | Area | Work Item |
|---|---|---|
| &#x2B1C; | Logging / observability | Add persistent log storage, streaming log subscriptions, richer structured payload schemas, and better kernel trap ingestion into the log pipeline |
| &#x2B1C; | Status / observability | Add richer service health reporting, subscription-based status monitoring, and shell/session status views |
| &#x2B1C; | Observability / desktop | Add desktop-facing logs, crash reports, service health surfaces, and app/runtime diagnostics on top of the current logging/status foundations |
| &#x2B1C; | Observability / developer | Add richer trace capture, performance/event timeline views, and operator/developer diagnostics for graphics, networking, runtimes, and builds |

## 18. Toward a More Complete OS

| Status | Area | Work Item |
|---|---|---|
| &#x2B1C; | Accounts / identity | Add user accounts, login/session ownership, identity switching, and user-scoped policy across storage, apps, runtimes, and services |
| &#x2B1C; | Backup / restore | Add backup, restore, migration, and state export/import flows for user data, apps, packages, and system configuration |
| &#x2B1C; | Printing / peripherals | Add printer/peripheral service contracts and desktop-facing peripheral management as hardware support broadens |
| &#x2B1C; | Peripherals / wireless | Add broader peripheral connectivity including Bluetooth input devices, wireless accessories, and consumer-device management flows |
| &#x2B1C; | Power / devices | Add suspend/resume, power policy, battery/thermal/device health reporting, and laptop-oriented desktop/system behavior |
| &#x2B1C; | Installation / onboarding | Add a real installer, setup/onboarding flows, recovery environment, and first-boot configuration experience |
| &#x2B1C; | Hardening / release | Add release engineering, reproducible builds, artifact signing, installer images, upgrade test matrices, and broader end-to-end validation for a near-complete OS |
