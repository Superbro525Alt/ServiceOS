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
| &#x2705; | Scheduling / memory / faults | Complete real CPU register context switching between unrelated kernel threads instead of the current bootstrap-oriented path |
| &#x2705; | Scheduling / memory / faults | Switch from the current boot-derived paging path to fully kernel-owned page tables (a fuller physical direct map beyond the current identity-offset mapping remains open) |
| &#x2705; | Scheduling / memory / faults | Extend the new memory-object info and range-based mapping syscalls into fuller VM control, including unmap, protection changes, virtual-memory queries, and less runtime-managed shared-range allocation |
| &#x2705; | Scheduling / memory / faults | Extend the new generic object inspection, event, and object-wait substrate into broader shared-memory IPC transport, timer/object coverage, and wider userspace adoption beyond the current task/event/channel/input/packet readiness set |
| &#x2705; | Scheduling / memory / faults | Add fast `SYSCALL/SYSRET` on x86_64 alongside the current interrupt-gate syscall path |
| &#x2705; | Scheduling / memory / faults | Add preemptive time-slice enforcement, CPU-local run queues, and SMP scheduling and interrupt routing |
| &#x2705; | Scheduling / memory / faults | Add alternate x86 timer sources: LAPIC timer is calibrated, armed, and now the active tick source, and an HPET driver is present and validated behind ACPI discovery for future wake scheduling |
| &#x2705; | Scheduling / memory / faults | Extend the current fault-state propagation, manager supervision, and desktop fault surfacing into richer user-fault upcalls and recovery policy |

## 2. Root Bootstrap and Core Service Management

| Status | Area | Work Item |
|---|---|---|
| &#x2705; | Root bootstrap | Root userspace service manager |
| &#x2705; | Root bootstrap | Persisted boot-store manifest index |
| &#x2705; | Root bootstrap | Dependency-ordered startup |
| &#x2705; | Root bootstrap | Capability-scoped startup grants |
| &#x2705; | Root bootstrap | Manager-mediated registration and discovery |
| &#x2705; | Packages / services | Add dynamic service installation, on-demand activation policy, and richer health-check definitions in manifests and package flows |
| &#x2705; | Observability / supervision | Add richer root-manager supervision and health policy without moving service lifecycle policy back into the kernel |
| &#x2B1C; | Bootstrap / service graph | Extend the current degraded startup and partial-graph recovery path with explicit boot-mode selection and operator-directed reduced graphs (partial: manager parses a boot-mode startup word (full/reduced/safe), applies core-set reduced graphs via transitive dependency/lookup closure with unit tests, and logs the selected mode; the kernel/platform side does not yet send the mode word, so full mode remains the boot default) |
| &#x2705; | Bootstrap / orchestration | Extend the current blocked-dependency diagnostics with cycle explanation and startup timing/ordering visibility for service bring-up (manager logs per-service ms/tick start-to-ready durations, an ordering line, total plus slowest-three summary at bring-up completion, and the concrete A&#x2192;B&#x2192;A path when a blocked-dependency chain closes a cycle) |

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
| &#x2705; | Services / policy | Extend the current service capability-template inspection and scoped on-demand delegation model with explicit revocation flows and richer delegated-capability review |
| &#x2705; | Services / status | Extend the current manager-structured graph and service status into broader per-service health/status surfaces across foundational services |
| &#x2705; | Services / resilience | Add service restart backoff, crash-loop detection, degraded-service state, and manager-facing escalation policy |

## 4. Storage, Filesystem, Packages, and Configuration

| Status | Area | Work Item |
|---|---|---|
| &#x2705; | Storage / filesystem | Replace the current in-memory writable overlay with persistent writable backing |
| &#x2705; | Storage / filesystem | Add writable storage and directory capabilities as first-class scoped authorities instead of broad ambient write access |
| &#x2705; | Storage / filesystem | Extended the mount inventory, composed namespace root, and relative directory-capability traversal with explicit capability-gated mount/unmount mutation (busy-check, atomic table updates, mount table persisted across reboot), multi-backend composition (persistent block-backed plus in-memory temp instances under longest-prefix resolution), and stat/find metadata-query protocols |
| &#x2705; | Storage / policy | Add broader user-home and storage policy, plus writable project/workspace directories and persistent build outputs |
| &#x2705; | Configuration | Add namespaced service configuration trees, write and update policy, and schema validation and migration |
| &#x2B1C; | Storage / indexing | Add file indexing, metadata queries, search primitives, and content discovery support for desktop and developer workflows |
| &#x2B1C; | Storage / sharing | Add explicit file-sharing, open-with, recent-files, and app-association policy on top of the core file and directory protocols |

## 5. Platform Tooling and Package Foundations

| Status | Area | Work Item |
|---|---|---|
| &#x2705; | Tooling | `shell-service` operator environment |
| &#x2705; | Tooling | `package-service` install, update, rollback, and activation coordination |
| &#x2B1C; | Packages | Extend the current network-backed repositories, trust metadata, writable install roots, install journals, and rollback policy with cryptographically signed feeds, key rotation, and stronger trust-root enforcement |
| &#x2B1C; | Packages / updates | Add whole-system image update workflows on top of the package/update foundation |
| &#x2705; | Packages / trust | Add package trust state, source provenance, signing/trust status, and rollback provenance inspection through the package-service contract |
| &#x2B1C; | Packages / policy | Extend the current package pinning and channel/ring policy with real staged rollout cohorts and richer per-source upgrade rules |
| &#x2705; | Packages / recovery | Add better interrupted-install recovery, consistency checking, garbage collection, and repair flows for partially applied updates |

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
| &#x2705; | Platform / targets | Add `platform/aarch64/virt` or equivalent clean ARM virtual target to keep ARM bring-up separate from Pi-specific board quirks |
| &#x2B1C; | Platform / targets | Add a second x86 platform target: `qemu-isa` (legacy BIOS/SeaBIOS PVH boot, serial-first) boots the full kernel and userspace bootstrap; final userspace entry transition still faults (see docs/handoff-qemu-isa.md). Splitting x86 PC interrupt-controller details out of `arch/x86_64` remains open |
| &#x2B1C; | Platform / targets | Add additional future-facing targets such as real x86 PC hardware and/or `riscv64/virt` once the current platform layering stabilizes |

## 7. Networking

| Status | Area | Work Item |
|---|---|---|
| &#x2705; | Networking | Grow the current networking surface beyond DHCP/DNS/ICMP/outbound TCP into UDP, inbound/listening transports, richer routing, multi-interface policy, and IPv6 |
| &#x2B1C; | Networking | Add long-lived resolver caching, richer DNS record support, firewalling, and broader network policy (partial: resolver caching landed — network-service now runs its own DNS-over-UDP client and wire codec in place of the smoltcp DNS socket, with a TTL-honoring positive/negative cache for A/AAAA/CNAME records (negative TTLs capped so operator-side fixes propagate), bounded CNAME chasing (8 hops) that follows both fully-appended in-packet chains and cross-query links, AAAA and TXT record parsing, distinct NXDOMAIN/SERVFAIL/NODATA/timeout detail codes appended to ResolveReply and a new typed ResolveEx contract, and resolver hit/miss counters carried in the trailing InterfaceStatusReply word; firewalling landed — an ordered first-match allow/deny rule table over protocol+direction+port with per-rule hit counters, inbound/outbound deny totals, and a settable default-inbound policy, enforced at outbound TCP connect / UDP send-to / ICMP ping and at inbound TCP accept / UDP receive, settable and queryable over the existing public network control channel via reserved tag values 0x80e–0x813 pending promotion into shared/abi; host unit tests cover cache TTL expiry, chain-depth bounds, and the firewall rule-match matrix; still open: IPv6 datagram plumbing beyond record parsing, richer policy sources (per-interface rules, address sets), promotion of the reserved tags into the shared ABI, and desktop-facing firewall/resolver state surfaces) |
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
| &#x2B1C; | Graphics | Grow the current damage-tracked multi-buffer graphics path into multiple outputs and sessions, explicit release/fence-style presentation sync, and broader compositor-side partial-present support |
| &#x2705; | Graphics | Grow the current shared-buffer graphics path into mapped or zero-copy presentation buffers and a broader client-render protocol for richer clients |
| &#x2B1C; | Graphics | Add richer display mode management and eventual GPU-accelerated composition without collapsing graphics policy into the desktop shell |
| &#x2705; | Input | Finish the move from receive-side polling to fully device-driven input wakeups; the platform IRQ path is active, but the current session/input stack still needs a one-shot nonblocking receive fallback to avoid missed-edge stalls |
| &#x2B1C; | Input | Add support for multiple physical input hosts and broader pointer/button routing beyond the current single-host desktop path |
| &#x2B1C; | Session / composition | Partial: multi-session registry landed in `session-service` (sessions carry ids and seat bindings), with a switch-active control operation that performs staged input-route detach, outgoing-focus teardown, and seat transfer; isolation policy enforces per-session app-surface membership on focus and kernel-input routing so apps of inactive sessions receive no pointer/key events, and session listing plus current-session queries ride the existing contracts (host unit tests cover the handoff state machine and isolation matrix); per-session desktop/compositor instances, operator login/session ownership, and true multi-seat hardware routing remain open |

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
| &#x2B1C; | Desktop shell | Finish the remaining shell follow-on work with gesture/snap/tiling policy, smoother window transitions, fuller open-with and default-association flows, and richer system-status surfaces on top of the current notification history and workspace model |
| &#x2B1C; | Desktop shell | Extend the current MRU task switcher, workspace shortcuts, and command-palette routing with smoother animation timing, broader gesture policy, and a more macOS-like interaction feel without collapsing shell logic into platform services |
| &#x2B1C; | Desktop shell | Extend the current app-focused command palette, launcher ranking, app switching history, and workspaces into document/content search and multi-monitor shell behavior |
| &#x2B1C; | Desktop shell | Finish the remaining inter-app desktop workflow work with drag-and-drop, broader app-intent/open-with/default-association policy, and richer file handoff beyond the current files-app open-path foundation and clipboard history |
| &#x2B1C; | Desktop shell | Extend the current launch-denied and package/runtime security surfaces into broader permissions-aware desktop UX for app authority, file/device access, notification policy, and runtime launches |
| &#x2B1C; | Desktop shell / wireless | Add desktop Wi-Fi UX for scanning, joining, saved networks, signal state, and connection troubleshooting |
| &#x2B1C; | Desktop shell / Bluetooth | Add desktop Bluetooth UX for pairing, device management, battery/state display, and audio/input routing |
| &#x2B1C; | Apps / UI | Add broader graphical application and toolkit/runtime layers on top of the current app and window foundations |

## 10. Terminal, Shell, Console, and Operator UX

| Status | Area | Work Item |
|---|---|---|
| &#x2B1C; | Shell / sessions | Add multiple shell and operator sessions, login/session ownership policy, package-installed command discovery, job control/pipelines, richer process environments, and richer operator history/status views |
| &#x2B1C; | Terminal | Split panes and explicit session profiles have landed on the current richer ANSI/VT base (per-pane sessions, focus/resize/close keybindings, profile-driven shell metadata); remaining expansion is remote terminal/SSH-backed sessions and durable profile persistence on top of the shared clipboard path, themes, and resize/reflow semantics |
| &#x2B1C; | Terminal | Add terminal session persistence/reattach, command bookmarking, session restore, and richer command/result inspection |
| &#x2B1C; | Console | Add graphical console surfaces and operator-session handoff/routing on top of the current serial console model |
| &#x2705; | Operator UX | Landed richer shell/operator diagnostics on the service contracts: `logs follow` streams a live filtered tail through log-service subscriptions and ends cleanly on Ctrl-C over console sessions (graphical terminal panes fall back to an idle timeout), `logs crashes` lists recent crash-shaped records by filtering the retained log ring client-side until log-service dispatches a dedicated crash-query tag, `status health` renders the status-service snapshot rollup as a formatted table, and `status svc <name>` / `ps app [name]` give structured per-service and per-app inspection |

## 11. Execution, Loading, Compatibility, and Runtimes

| Status | Area | Work Item |
|---|---|---|
| &#x2705; | Runtime foundation | Package-delivered `runtime-service` for explicit compatibility/runtime environments |
| &#x2705; | Runtime foundation | Environment creation, inspection, run launch, and teardown through a real userspace service contract |
| &#x2705; | Runtime foundation | Explicit mount and variable mapping for the first `posix` runtime profile |
| &#x2705; | Runtime foundation | Manager-mediated launch of runtime-hosted workloads instead of shell-owned compatibility shortcuts |
| &#x2705; | Runtime foundation | Shared shell/terminal operator integration for runtime inspection and launch |
| &#x2B1C; | Loading / execution | Grow the current flat-image-plus-ELF native loader into dependency loading, richer executable/runtime policy, and broader native image format support |
| &#x2705; | Loading / execution | Add a general process loader for user-supplied images instead of only manager-owned stored images |
| &#x2B1C; | Compatibility | Grow the current compatibility/runtime foundation beyond hosted `posix` environments into Linux-oriented ABI expansion, arbitrary ELF execution, richer runtime packaging, and desktop launch UX for runtime-hosted apps |
| &#x2B1C; | Compatibility / security | Add explicit capability grants for network, graphics, input, and audio to compatibility workloads |
| &#x2B1C; | Compatibility | Add Windows runtime support and broader cross-platform application execution |
| &#x2B1C; | Compatibility / security | Add stronger sandboxing and container-style isolation for compatibility workloads |
| &#x2B1C; | Runtime UX | Add desktop-facing runtime launch surfaces, runtime state inspection, and app/runtime distinction UX for native versus hosted applications |

## 12. Audio and Media

| Status | Area | Work Item |
|---|---|---|
| &#x2B1C; | Audio | Grow the current audio surface beyond tone playback: PCM output streams with format negotiation (U8/S16/S32/F32, mono/stereo, rate table), chunked IPC writes (blocking/nonblocking), N-stream mixing with clipping protection, and per-stream + master volume/mute landed on a mixed-PCM null sink validated by boot selftest and host unit tests; the PC-speaker endpoint remains tone-only, and shared-buffer output, audible PCM backends, capture streams, per-app policy, and notification/media controls remain open |
| &#x2B1C; | Audio / hardware | Add broader hardware audio backends beyond the current wired/emulated paths, including Bluetooth audio and future USB audio paths |
| &#x2B1C; | Audio / Bluetooth | Add Bluetooth audio output/input support, pairing integration, and endpoint routing on top of the broader audio platform |
| &#x2B1C; | Media | Add codecs, containers, richer media pipelines, DMA-safe memory-object policy, and broader hardware backends beyond the current QEMU PC speaker path |
| &#x2B1C; | Audio / UX | Partial: desktop-facing volume and endpoint status surfaced — desktop-shell gained a media overlay (Alt+M or command palette) that honestly reports output endpoint presence/state (including no-endpoint and audio-unavailable states), master volume/mute with keyboard control wired to the audio-service set-master-volume contract (`EndpointVolumeSet`), and active/listed PCM stream counts; the settings System page shows live PCM stream counts beside endpoint state. Master volume is set-only in the ABI (no read-back tag), so the shell displays its last applied value, and per-stream formats are visible only at the sink level; true endpoint selection, per-app audio policy, and notification/media playback integration remain open |
| &#x2B1C; | Media / apps | Partial: first-party media preview basics landed as a read-only desktop-shell media surface listing currently-active PCM streams (slot, direction, state, session, endpoint, frequency) alongside mixed-sink counters (frames mixed, FNV checksum) from the null-sink drain-side stats; codecs, containers, richer media pipelines, DMA-safe memory-object policy, broader hardware backends beyond the current QEMU PC speaker path, and deeper files/notifications/runtime app integration remain open |

## 13. Developer Tooling and Workflows

| Status | Area | Work Item |
|---|---|---|
| &#x2705; | Developer tooling | Package-delivered `developer-service` for toolchain, workspace, build-job, and artifact management |
| &#x2705; | Developer tooling | Packaged toolchain descriptors for native, Linux, Windows, and honest remote-only macOS target metadata |
| &#x2705; | Developer tooling | Packaged workspace descriptors and sample source payloads for the first cross-target workflow |
| &#x2705; | Developer tooling | Manager-mediated launch of transient `cross-builder-tool` workers instead of shell-owned build shortcuts |
| &#x2705; | Developer tooling | Shared shell/terminal operator integration for toolchain inspection, build submission, job inspection, and artifact export |
| &#x2B1C; | Developer workflows | Grow the current developer tooling foundation beyond packaged sample workspaces into broader SDK/toolchain distribution, richer language ecosystems, and runtime-aware build/run workflows (partial: build-worker sandboxing groundwork landed — developer-service now derives an explicit per-job capability manifest from the workspace descriptor plus toolchain SDK root with network denied, records the allow decision on the job record, hands the permission set to `cross-builder-tool` at launch, and the worker echoes `worker sandbox:` and cleanly fails out-of-scope jobs with a distinct sandbox-denied report status; in-service toolchain registry landed — developer-service derives family (rust/gcc/llvm/native), dotted version, newest-first per-family rank, and a live storage presence probe for each packaged descriptor, serves them as trailing fields on the existing toolchain list/info replies, and refuses builds whose installed toolchain SDK root no longer resolves; runtime-aware routing landed at the decision layer — `BuildRequest` accepts an optional runtime profile tag, the service resolves it against active runtime-service environments over the runtime env-list contract and records the route on the job record with a `worker route:` echo, falling back to the existing direct worker spawn whenever no tag is given or no matching ready environment answers. Distribution of additional SDK payloads and executing builds inside routed environments remain open) |
| &#x2B1C; | Developer workflows / infra | Add remote build farms and remote macOS build/sign/notarization integration on top of the current honest remote-only target model |
| &#x2B1C; | Developer UX | Add IDE/editor integration and desktop-facing developer workflow UX without bypassing the shared shell/runtime path |
| &#x2B1C; | Developer UX | Add project browser, artifact viewer, debugger/profiler hooks, and richer build/test/run surfaces in the desktop environment |
| &#x2B1C; | Developer trust / policy | Add explicit developer-tool permissions, workspace trust policy, and project/runtime authority review surfaces |

## 14. Permissions, Trust, and Security UX

| Status | Area | Work Item |
|---|---|---|
| &#x2705; | Permissions | Add clearer app permissions and capability review surfaces for native apps, runtime-hosted apps, and developer tools |
| &#x2B1C; | Runtime permissions | Service-side review flows landed on the existing contracts: pending-request queue query (runtime `EnvList` filtered to `PendingApproval`), approve/deny plus approve-subset grants through the existing runtime decision contract with per-decision approval audit records carrying the granted mask, kind-filtered audit inspection queries on both the runtime and security services, and host unit tests for subset validation, decision decoding, and audit roundtrip; this also fixed a decision-word decode mismatch that landed desktop approvals as denials. Actual desktop prompt UX beyond the current settings buttons, packaged runtime profiles requesting sensitive capabilities by default, and persisted grant history remain open |
| &#x2B1C; | Trust / signing UX | Extend the current package/repository trust UI and operator inspection into runtime/developer-artifact trust surfacing plus stronger cryptographic signing and trust-root enforcement |
| &#x2705; | Package trust UI | Add package trust UI showing provenance, trust/signing state, update source, and rollback provenance in both terminal and desktop flows |
| &#x2B1C; | Desktop security surfaces | Extend the current settings-based security review page into broader privileged-action, trust-warning, and authority-escalation flows across software installation, runtime launch, and document/open flows |
| &#x2B1C; | Security UX | Extend the current shell denial messages, manager launch-denied status, and audit inspection into broader desktop-facing denial/error explanations across apps and runtime-hosted workloads |
| &#x2705; | Security policy | Add stronger permission editing/review flows, revocation UX, and capability grant history without moving policy into the shell or UI code |
| &#x2B1C; | Security foundations | Build on the current security audit and runtime approval foundation with stronger signing/key management, trust roots, and broader sandbox policy expansion |

## 15. Software Distribution and App Ecosystem

| Status | Area | Work Item |
|---|---|---|
| &#x2705; | Software center | Add a software center / app-store style GUI on top of the package/update foundation |
| &#x2B1C; | Installer / updater UX | Partial: shell/operator installer-updater UX landed on the package-service pair — `pkg install/update <name> [version] [@source]` accepts an explicit source with a trust preview (mode, pinned digest, sync state) and a `--yes` confirmation gate for non-boot-trusted sources; install/update/rollback replies carry per-phase progress counters (resolve/materialize/verify/activate/persist step counts plus percent) that the shell renders; rollback prints a previous→restored version summary with trigger; maintenance replies expose operation-journal status (stale-since-boot detection with startup Warn telemetry) and a `pkg recover` action resumes or discards interrupted installs/updates/rollbacks; host unit tests cover progress phase math, journal staleness classification, and the source-selection grammar. Mutation handlers no longer propagate request errors into service exit. Live streaming progress over the log stream, software-center/desktop surfaces for these flows, and richer trust explanations remain open |
| &#x2B1C; | App lifecycle | Complete third-party app lifecycle UX by wiring installed packages into launch surfaces, update/remove visibility, permissions review handoff, uninstall cleanup visibility, and file/open-with association policy |
| &#x2705; | App ecosystem | Add richer catalog search with tiered name/description/keyword ranking, a category filter view, and per-package host/target compatibility surfacing backed by software-center catalog metadata; screenshot display and recommendations remain open |
| &#x2B1C; | App distribution | Add desktop-facing third-party repository onboarding, trust review, side-loading policy, and package/runtime compatibility warnings on top of the current shell-driven repository registration path |
| &#x2B1C; | App policy | Add per-app default associations, intent/open-with policy, recent apps/documents, and uninstall cleanup behavior |

## 16. Desktop Polish and Advanced Interaction

| Status | Area | Work Item |
|---|---|---|
| &#x2B1C; | Desktop polish | Build on the current MRU task switcher and workspace overlays with smoother animations, gesture handling, and a more macOS-like desktop feel |
| &#x2B1C; | Desktop polish | Build on the current notification history and quick-focus action path with richer quick actions and broader permissions-aware desktop surfaces |
| &#x2B1C; | Desktop polish | Add smoother windowing behavior, animation timing, shadowing, transitions, and interaction polish without collapsing shell logic into compositor policy |
| &#x2B1C; | Desktop polish | Extend the current command palette/search UX, launcher ranking, and workspace shortcuts with broader global action routing and cross-app interaction patterns |
| &#x2B1C; | Desktop polish | Finish the remaining shell affordance work with drag-and-drop, desktop gestures, hot corners, and richer clipboard/file handoff beyond the current clipboard history foundation |
| &#x2B1C; | Desktop polish | Build on the current keyboard-first shell overlays with high-contrast settings, zoom or magnification hooks, and broader assistive interaction hooks |

## 17. Observability, Logging, and Status

| Status | Area | Work Item |
|---|---|---|
| &#x2705; | Logging / observability | Add persistent log storage, streaming log subscriptions, richer structured payload schemas, and better kernel trap ingestion into the log pipeline |
| &#x2705; | Status / observability | Add richer service health reporting, subscription-based status monitoring, and shell/session status views |
| &#x2705; | Observability / desktop | Add crash-record capture and query in the log service and a system-health rollup in the status snapshot reply (total services, counts by state, degraded/restarting lists, worst restart offenders) on top of the current logging/status foundations; desktop-facing log/crash viewer surfaces and app/runtime diagnostics remain open |
| &#x2B1C; | Observability / developer | Add richer trace capture, performance/event timeline views, and operator/developer diagnostics for graphics, networking, runtimes, and builds |

## 18. Toward a More Complete OS

| Status | Area | Work Item |
|---|---|---|
| &#x2B1C; | Accounts / identity | Partial: service-side foundation landed in `account-service` — account store (id/name/display-name, per-account salted credential hashes using an honestly non-cryptographic FNV-based KDF) persisted via storage-service contracts; login/logout claim session ownership over session-service session ids and identity switching re-binds the active claim across sessions mirroring handoff semantics; per-account default capability grant sets recorded and exposed for future enforcement points; host unit tests cover hashing, the login state machine, switch semantics, policy defaults, and store serialization. Activation is manual (image built into the boot store, spawned by path via stored-image launch; not in the default boot graph and not registered under a named `ServiceId` pending a shared-ABI slot). Graphical login UI, a real password KDF, and enforcement across storage, apps, runtimes, and services remain open |
| &#x2B1C; | Backup / restore | Add backup, restore, migration, and state export/import flows for user data, apps, packages, and system configuration |
| &#x2B1C; | Printing / peripherals | Add printer/peripheral service contracts and desktop-facing peripheral management as hardware support broadens |
| &#x2B1C; | Peripherals / wireless | Add broader peripheral connectivity including Bluetooth input devices, wireless accessories, and consumer-device management flows |
| &#x2B1C; | Power / devices | Add suspend/resume, power policy, battery/thermal/device health reporting, and laptop-oriented desktop/system behavior |
| &#x2B1C; | Installation / onboarding | Add a real installer, setup/onboarding flows, recovery environment, and first-boot configuration experience |
| &#x2B1C; | Hardening / release | Add release engineering, reproducible builds, artifact signing, installer images, upgrade test matrices, and broader end-to-end validation for a near-complete OS |
