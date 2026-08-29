# ServiceOS — As-Built System Documentation

Status: as-built snapshot of `main`. Every named syscall, tag, path, and
constant below is grep-verifiable in this tree. Companion docs:
`docs/architecture.md`, `docs/services.md`, `docs/boot-flow.md`,
`docs/platforms.md`, `docs/handoff-qemu-isa.md`, `docs/roadmap.md`.

## 1. Overview

ServiceOS is an experimental service-oriented operating system written in Rust
(edition 2024, `no_std` throughout kernel and userspace, Rust ≥ 1.85). It is a
capability-based, microkernel-style OS: the kernel provides mechanisms only —
address spaces, threads, kernel objects, handles/capabilities, channel and pipe
IPC, timers, traps, and syscalls — while all platform behavior (storage,
logging, configuration, networking, graphics, audio, packaging, security
policy, shell, desktop) lives in userspace services coordinated by a root
service manager.

Design invariants:

- authority flows through handles held in per-task capability spaces; there is
  no ambient authority
- service discovery is manager-mediated and identity-based; knowing a service
  name grants nothing
- faults terminate the faulting task, not the machine (per-service isolation)
- QEMU-first bring-up without locking architecture to VM-only assumptions

### Platforms

| Platform | ISA | Boot model | State |
|---|---|---|---|
| `qemu-virtio` | x86_64 | UEFI/OVMF (`BOOTX64.EFI`) + VirtIO | primary target; full desktop graph |
| `qemu-isa` | x86_64 | legacy BIOS via SeaBIOS PVH ELF note | kernel init fully succeeds; first userspace entry transition still faults (`docs/handoff-qemu-isa.md`) |
| `virt` | aarch64 | QEMU `virt` machine, arm64 Image + DTB | full graph: framebuffer, VirtIO input/net/block, GIC, device selftests |
| `raspi5` | aarch64 | Raspberry Pi firmware, native `kernel8.img` | serial-first foundational graph (storage, console, config, log, status, package, shell) |
| `riscv64-virt` | riscv64 | OpenSBI `-bios default` payload at `0x80200000` | skeleton: SBI console banner, hang-loop stvec handler, one-shot SBI timer; no MMU/userspace yet |

## 2. Architecture Layers

```text
kernel/core            generic mechanisms: objects, capabilities, scheduler,
                       memory, IPC, syscalls, loader, fault upcalls
  -> arch/<isa>        CPU privilege, MMU, trap/syscall entry, user transition
      arch/x86_64      arch/aarch64      arch/riscv64
  -> platform/<plat>   firmware parsing, BootInfo normalization, devices
      x86_64/qemu_virtio   x86_64/qemu_isa
      aarch64/raspi5       aarch64/virt       riscv64/virt
  -> userspace         root-manager -> service graph -> apps/tools
```

Support crates: `shared/abi` (syscall, IPC, service, graphics, network,
package, storage, audio ABIs), `shared/bundle` (service/package/boot-store
bundle format), `support/xtask` (platform-aware build/run orchestration),
`userspace/catalog` (image catalog staged into the boot store).

Kernel/core module map (`kernel/core/src/`): `object/` (registry, rights,
pipes, events), `capability/`, `ipc/` (channels), `task/` (threads, scheduler,
kernel context), `memory/` (phys frames, heap, layout, manager, pressure, OOM),
`syscall/` (dispatch + handler families), `user/` (loader v2, spawn, runtime
images), `fault.rs` (registry + upcalls), `interrupts.rs`, plus backend-neutral
contracts for `display/`, `input/`, `network/` (incl. zero-copy `ring.rs`),
`audio/`, `block/`, `bootstrap/`, `time/`.

Userspace programs (`userspace/programs/`) — every binary, one line each:

| Program | Purpose |
|---|---|
| `root-manager` | first coordinator: dependency-ordered graph, capability grants, lookup mediation, supervision, boot modes |
| `runtime` | `serviceos-userspace-runtime`: shared user-program library wrapping all service contracts/syscalls |
| `ui` | `serviceos-desktop-ui`: shared desktop UI widgets/glyphs for shell and apps |
| `storage-service` | mounts immutable boot store + mutable namespaces; scoped directory/blob capabilities; index + fsck |
| `console-service` | userspace route to kernel debug sink; line-oriented operator serial sessions |
| `config-service` | typed config values from persisted `config/system.cfg` blob |
| `log-service` | durable filtered log sink; persists records; live subscriptions; drains kernel trap/pressure records |
| `status-service` | per-service health table; query/list/subscribe monitoring; heartbeats |
| `network-service` | interface/IP policy over explicit packet-interface cap; DNS, firewall, discovery, TCP streams |
| `audio-service` | endpoint + playback/capture PCM streams over explicit audio-endpoint cap (QEMU speaker backend) |
| `clipboard-service` | shared text clipboard read/write contract for desktop apps |
| `security-service` | native app launch-policy review/override, runtime approval state, audit history |
| `graphics-service` | display outputs, surfaces, compositor: damage, fences, partial present, mirror output |
| `session-service` | session identity/focus policy; physical input ingress for the active session |
| `terminal-service` | PTY-like terminal sessions for graphical terminal hosting; remote links |
| `desktop-shell-service` | product shell: chrome, launcher, windows, workspaces, app lifecycle, notifications |
| `shell-service` | serial operator shell; sessions, login, job control, pipelines over real contracts |
| `package-service` | repositories, trust/provenance/signing, install/update/remove/rollback, sysupdate model |
| `setup-wizard` | serial-driven first-boot wizard (hostname/timezone/admin account), done-marker skip |
| `account-service` | account store + login/identity state machine (manual activation) |
| `backup-service` | versioned snapshot/restore of system state under `backups/` (manual; recovery core) |
| `peripheral-service` | device registry classify/attach/detach + printer stub (manual activation) |
| `power-service` | sleep-inhibit refcount policy, suspend groundwork, battery/thermal v0 (manual activation) |
| `announce-service` | package-provided demo service proving package activation/rollback |
| `runtime-service` | compatibility environments: mounts, variables, sandbox grant matrix, guest exec (package-delivered) |
| `developer-service` | toolchains/workspaces/build jobs/artifacts (package-delivered) |
| `posix-host-tool` | first runtime-hosted transient workload image |
| `cross-builder-tool` | transient cross-build worker launched by developer-service |
| `monitor-app` | graphical status/log monitor; auto-launched at desktop bring-up |
| `settings-app` | graphical settings client (config/audio/security/state) |
| `files-app` | graphical file browser over storage namespace capabilities |
| `terminal-app` | graphical terminal front-end over terminal-service |
| `software-center-app` | package catalog browser/installer UI over package-service |
| `media-app` | media playback client over audio/graphics contracts |
| `sysinfo-tool` | transient system-info dump tool |

Service identities are fixed in `shared/abi/src/bootstrap.rs`
(`ServiceId`: RootManager=1 … Security=19, SetupWizard=20, Backup=21;
`ServiceImageId` adds MediaApp=31, SetupWizard=33, BackupService=34).
Manual-activation services (`account`, `peripheral`, `power`) intentionally
have no `ServiceId` slot yet and spawn via the manager's stored-image path.

## 3. Subsystem Deep-Dives

### 3.1 Scheduling, preemption, SMP

Per-CPU runnable queues with work-stealing (`kernel/core/src/task/scheduler.rs`):
an idle CPU steals up to a bounded batch from victims, either immediately
(steal-on-empty) or once a queued thread has sat runnable past a stealability
delay; round-robin scan cursors avoid herd effects. Steal statistics
(attempts, rebalance moves) are emitted periodically through a registered
sink. With fewer than two registered CPUs (`register_balancing_cpu_count`)
steal/balance passes are disabled entirely so single-core boots stay
byte-deterministic. Preemption is timer-driven with a `preemption_pending`
flag drained at safe points; deadline wakeups ride the monotonic clock.

### 3.2 Memory

`memory/phys.rs` discovers usable regions from `BootInfo`, keeps a reclaim
pool (boot-services pages are reclaimed after heap bootstrap), and hands out
frames. `memory/heap.rs` implements the kernel free-list heap in the upper
canonical half with free-byte accounting. `memory/pressure.rs` classifies
usable-frame and heap headroom into Normal/Tight/Critical against permille
threshold constants, tracks transitions in a bounded ring, notifies listeners,
and mirrors transitions into the kernel event ring (surfaced by log/status
services as `KernelPressureChanged`). `memory/oom.rs` implements the OOM
policy: on allocation failure it reclassifies pressure, selects the largest-
footprint reclaimable task (root-manager/console/shell-named and
non-reclaimable candidates protected; ties broken toward the lowest task id),
terminates it fault-style with exit code `0x4f4f4d00`, deschedules its
threads, returns charged frames, and retries allocation exactly once;
protected-set exhaustion panics honestly. Known gap: per-task frame-charging
hooks exist but no call site charges yet (see §8).

User address spaces are built per task; `MemoryMap/MemoryMapRange/
MemoryProtect/MemoryQuery/MemoryUnmap` (syscalls 27, 33, 42–44) expose mapping
with W^X enforcement; the loader applies per-segment W^X/NX (`GNU_STACK`)
policy.

### 3.3 IPC: channels, pipes, object waits

Channels are bounded message queues between connected handles with explicit
transfer rules (rights-reduced duplication; send-only masks are not
forwardable through the manager). Kernel `Pipe` objects (syscalls
`PipeCreate/PipeRead/PipeWrite` = 49–51) are 64 KiB bounded rings returning
reader/writer handle pairs; reads require READ, writes WRITE, both are
WAIT-able through the object-wait substrate (`ObjectWait` syscall 38, plus
events 34–36). Blocking parks the caller until data/EOF or ring space;
`PIPE_FLAG_NONBLOCK` degrades both sides to immediate queue-empty errors. EOF
fires when the last writer handle closes (close/duplicate hooks keep per-side
refcounts); `BrokenPipe` once the last reader is gone. The shell's
`cmdA | cmdB` pipelines cross a real kernel pipe per stage boundary.

### 3.4 Faults and upcalls

`kernel/core/src/fault.rs` maintains a registry keyed by `FaultType`
(InvalidOpcode, PageFault, GeneralProtection, Breakpoint, Other(u8)); tasks
register handlers via `FaultHandlerRegister/Unregister` (syscalls 45–46) with
a notification endpoint. Two dispositions exist per task class: terminate
(task exits `TaskExitStatus::Faulted`, supervisor decides restart/backoff via
the root-manager recovery engine) and supervisor upcall (fault record delivered
to a registered handler endpoint). Log-service drains kernel trap records into
the structured log stream.

### 3.5 Loader v2 (ELF + dynamic linking)

`kernel/core/src/user/loader.rs` resolves executables from the boot store with
fallback order flat v2 → flat v1 → raw ELF64. The ELF64 path parses PT_LOAD
segments with user-window containment and W^X/NX policy, supports `ET_EXEC`
and `ET_DYN` (PIE: deterministic page-aligned base at the bottom of the image
window, base-relative entry/vaddrs), and reads `PT_DYNAMIC` for `.rela.dyn`
(`R_X86_64_RELATIVE`), `.dynsym`/`.dynstr` + SysV `DT_HASH` symbol lookup, and
`R_X86_64_GLOB_DAT`/`R_X86_64_JUMP_SLOT` fixups applied after all images are
mapped. Flat-image dependencies may be ET_DYN shared objects loaded at
companion bases; defined global/weak exports register into a fixed-capacity
symbol namespace where later registrations override equal-strength ones (main
image registers last, so it wins). Unresolved weak symbols resolve to 0;
unresolved strong symbols fail the load (`UnresolvedSymbol`). Still open:
PT_INTERP/ld.so execution, lazy PLT binding, relro (§8). `TaskLoadedLibraries`
(syscall 47) reports the module set.

### 3.6 Input pipeline

Backends enumerate devices into kernel input objects; events carry a source id
so multi-host topologies survive transport — secondary-host tagging is tested
in `kernel/core/src/input/mod.rs` (stale-source events from absent hosts are
skipped and the queue keeps flowing). `session-service` consumes the bootstrap
input-source capability and owns physical pointer/keyboard ingress for the
active graphical session, forwarding into the desktop interaction contract;
`desktop-shell-service` does hit-testing, focus routing, global shortcuts, and
app delivery. Raw receive path: `InputSourceInfo/InputSourceReceive`
(syscalls 20–21).

### 3.7 Graphics composition

`graphics-service` owns the display-output capability, per-surface handles,
and the compositor (`compose.rs`, `outputs.rs`, `fence.rs`). Present paths:
full-frame `DisplayOutputPresent` and damage-tracked
`DisplayOutputPresentDamage` (syscall 41); the compositor keeps a presented-
frame shadow, skips byte-identical regions, merges changed rows into disjoint
scanline bands, and flushes bands only when the diff is <50% of frame bytes
(fallback to whole-clip otherwise), accumulating savings counters per output.
Every present issues a monotonic frame-counter fence token carried in
`PresentBufferReply`; clients wait through service-local control ops `0x912`
(request token+timeout)/`0x913` (reply) backed by a parked-waiter list reaped
after each present. Destructive surface closes during a pending frame defer
until the covering present. Outputs: the boot framebuffer is primary plus one
on-demand memory-backed virtual mirror output (control op `0x910`,
nearest-neighbour scaled blit with per-output stats). Mode management exists
as an honest contract (`display/mode.rs`): enumerated single-entry default for
boot framebuffers; real mode-setting hardware is deferred (§8).

### 3.8 Storage

`storage-service` mounts the immutable boot store handed off by firmware
through the kernel, plus mutable namespaces under a composed root: mount table
(max 16 entries) with kinds Boot/Persistent/Ephemeral/Temp
(`shared/abi/src/storage.rs::StorageMountKind`); live namespaces include
`state/` (persistent policy/account/security/wizard data), `data/` (user
data), `backups/`. Callers traverse via scoped directory capabilities with
relative traversal — never a root-handle string open. Files become explicit
blob capabilities via exact-path open. `index.rs` maintains a content index
(prefix listing, size-bounded queries); `fsck.rs` validates namespace
consistency (orphaned entries, path integrity) with its own selftest suite;
`persistent.rs` implements durable mutable-store persistence; `selftest.rs`
runs boot-time read/write verification (`selftest file-written bytes=` marker).

### 3.9 Networking

`network-service` holds IP-level policy behind one startup-granted packet-
interface capability: DHCP with static fallback from config, static hosts
mappings from `config/hosts.cfg`, ICMP, outbound TCP stream sessions, and an
in-house DNS-over-UDP client with a TTL-honoring positive/negative cache for
A/AAAA/CNAME (bounded CNAME chasing at 8 hops, distinct NXDOMAIN/SERVFAIL/
NODATA/timeout codes, `ResolveEx` typed contract, hit/miss counters). Firewall:
ordered first-match allow/deny rules over protocol+direction+port with per-rule
hit counters, inbound/outbound deny totals, settable default-inbound policy
(reserved channel tags 0x80e–0x813). Host naming/discovery: hostname get/set +
`hostname=`/`mdns=`/`discovery=` config lines, an mDNS-LITE responder on UDP
5353 answering `<hostname>.local` unicast queries (honest subset: no probing,
no SRV/TXT/PTR/DNS-SD), a UDP beacon on port 41453 announcing/querying peers,
continuous-ping with RTT stats, ARP-snooped neighbor dumps, and a port-scan-
self helper (tags 0x814–0x821). Zero-copy rings: `PacketInterfaceRingSetup`
(52) negotiates a memory-object-backed RX ring (header page + slot pages,
free-running head/tail counters, drop-oldest overflow); TX mirror
`PacketInterfaceTxRingSetup`/`Flush` (53–54) lets the service publish frames
into slots with credit accounting and a stall watchdog that reverts to the
legacy copied path after 8 undrained doorbells. Honest limits: one copy
remains on each side (device→ring RX, slot→descriptor TX), and the live TX
path currently reverts due to a cross-view visibility gap being root-caused
(docs/roadmap.md §7).

### 3.10 Audio

`audio-service` consumes the bootstrap audio-endpoint capability and serves
endpoint info/playtone/stop plus PCM streams: playback (`AudioEndpointPcmWrite`,
syscall 48) and capture (shared `abi/audio_capture.rs` wire format) with
per-stream state, sample-format handling, resample length math
(`pcm_resampled_len`), and mixed-PCM null-sink counters surfaced as extension
words. Streams associate with session ids without collapsing session policy
into the backend. Host side: QEMU PC-speaker backend today; virtio-sound PCI
playback attached when `SERVICEOS_AUDIO=1`.

### 3.11 Security model

Layered, explicitly not ambient:

- capabilities/handles: every authority is a kernel handle with rights;
  startup grants are narrow (e.g. only graphics-service gets the display
  capability; only network-service gets the packet interface)
- `security-service`: native app launch-policy review/override persisted under
  `state/security/launch-policy.cfg` (bounded policy + audit tables), runtime
  environment approval state, repository/package trust inspection, audit trail
  (`SecurityAuditKind` records); manager retains the actual launch decision
- runtime sandbox matrix: per-environment device-class grants
  (network/graphics/input/audio) default-deny, flowing only through the
  pending-approval → decision path into the audit trail
- package trust: `package-service/src/signing.rs` keystore with trusted keys
  per source, enrollment, rotation with retire windows, feed signature
  verdicts (unsigned / pinned-key match) driving install policy
- honesty markers: account credential KDF and backup blob checksums are
  FNV-based non-cryptographic placeholders pending real crypto (§8)

### 3.12 Update & rollback

Packages: repositories with channels (stable/beta/canary) and rings
(production/preview/testing), pinning, provenance inspection, history,
verify/repair/gc, and rollback of the active manifest version. System update
model lives in `package-service/src/sysupdate_model.rs` + `sysupdate_ops.rs`.
End-to-end upgrade durability is proven by `cargo xtask test-upgrade`
(boot → upgrade → reboot cycle verifying storage persistence markers survive
rebuilds between boots). `cargo xtask release` builds images for all
registered platforms and emits a `RELEASE-MANIFEST.json`.

## 4. Boot Flows

### 4.1 qemu-virtio (primary, x86_64 UEFI)

OVMF → `BOOTX64.EFI` (platform image crate) → early serial init → read
`\serviceos\bootstore.bin` from the ESP → capture ACPI RSDP →
`ExitBootServices` → normalize UEFI memory map + boot store into `BootInfo` →
generic kernel init (heap, objects, IPC, scheduler, syscalls, timer) → create
bootstrap channel + boot-store object → build root address space → load root-
manager from boot store → enter ring 3 → root-manager starts storage, loads
persisted manifests, brings up the dependency-ordered service graph → desktop.

### 4.2 qemu-isa (x86_64 BIOS/PVH)

SeaBIOS PVH ELF note → `mb_entry.S` long-mode trampoline (identity 2 MiB
pages, NXE+LME) → PVH v1 start_info memmap parsed into `BootInfo` → full
kernel init succeeds (LAPIC timer, HPET, SMP probe, PIC/PIT, kthread
self-switches) → bootstrap reaches "entering userspace executor" → **known
bug**: first `resume_user` IRETQ raises #GP(0xff50) with stale BIOS IVT bytes
in the restored context; tracked with hypotheses in
`docs/handoff-qemu-isa.md`.

### 4.3 virt (aarch64)

QEMU `-M virt` loads the arm64 Image; DTB parse normalizes memory and stdout
UART; PL011 up; GIC + timer; MMU on; generic kernel init; embedded boot store
resolves userspace; root-manager starts the full graph including graphics/
input/net/block backends (`platform/aarch64/virt/src/`: `framebuffer.rs`,
`input.rs`, `net.rs`, `block.rs`, `virtio.rs`, `selftest.rs`). Device
selftests run at bootstrap ("device selftests starting").

### 4.4 raspi5 (aarch64, serial-first)

Raspberry Pi firmware → `kernel8.img` → EL2→EL1 drop → DTB memory/stdout
discovery → PL011 → page tables + MMU → EL1 vectors → generic kernel init →
embedded boot store → EL0 root-manager → serial-first foundational graph
(storage, console, config, log, status, package, shell). Framebuffer/graphical
follow-ons remain open (roadmap §6).

### 4.5 riscv64-virt (skeleton)

OpenSBI default bios → payload at `0x80200000` → SBI legacy-console banner →
stvec hang-loop handler with cause/sepc logging → one-shot SBI TIME timer →
park hart 0. No MMU/paging, trap dispatch, drivers, or userspace yet.

### 4.6 First-boot wizard, recovery, boot modes

On the first boot the eager `setup-wizard` runs a serial step machine
(hostname via config-service, timezone file, admin account provisioned into
the persisted account store), writes `state/setup-wizard/firstboot.done`, and
exits; later boots see one task spawn and a skip line. Per-step serial input
windows (400 ticks, re-armed per keystroke) let interactive operators drive
setup while headless boots fall through to documented defaults.

Boot modes are selected at build time and passed as a word to root-manager
(`userspace/programs/root-manager/src/bootmode.rs`). `support/xtask-core`
validates `SERVICEOS_BOOT_MODE` before build/image staging, rejects anything
outside `full|reduced|safe|recovery`, and leaves unset boots on the byte-
identical full path:

| Word | Mode | Core set kept |
|---|---|---|
| 0 | full | entire graph |
| 1 | reduced | storage, console, config, log, status, shell, package, network, security |
| 2 | safe | storage, console, config, log, status |
| 3 | recovery | storage, console, backup (on-demand activation forced) |

All four graph-capable loaders (`qemu-virtio`, `qemu-isa`, `virt`, `raspi5`)
compile the selected word into the root-manager startup message. `cargo xtask
recover` is the recovery convenience path and simply sets
`SERVICEOS_BOOT_MODE=recovery`. Recovery gives operators persistent storage, a
serial console, and backup-service export/restore. Independent of boot mode,
the root-manager recovery engine supervises the running graph: crash-loop
accounting against `CRASH_LOOP_LIMIT` yields Restart, SupervisorCall, or
FailStop decisions (`src/recovery.rs`, applied in `src/graph.rs`).

## 5. Operator Quick-Reference

Shell groups (`shell-service`, serial-first, also hosted by terminal-service):
`help`, `services`/`service <name>`/`restart <name>`, `logs [count]`,
`config`, `store ls|mounts|mkdir|write|rm`, `cat`, `status`,
`net ifaces|route|sockets|resolve|ping|http`, `gfx outputs|surfaces|sessions|
focus`, `desktop status|apps|windows|workspace*|notifications|launch|focus|
next|close|minimize|restore|maximize|move|resize|click|notify|open`,
`run image <path>`/`run sysinfo`, `pkg list|catalog|repos|repo add|repo sync|
info|install|update|remove|rollback|history|provenance|policy|pin|channel|
ring|verify|repair|gc`, `runtime envs|create|inspect|mounts|vars|runs|launch|
destroy`, `security apps|app|runtimes|runtime|repos|package|workspace|audit`,
`dev toolchains|toolchain|workspaces|workspace|build|jobs|artifact`, plus
session/job features: `sessions`, `history [count]`, `login`/`whoami`/
`logout`/`su`, `command &` background jobs with `jobs`/`fg <id>`, and kernel-
pipe pipelines (`cmdA | cmdB`, opt-in consuming stages `filter`/`count`/`cat`,
four-stage cap).

Keybindings (desktop-shell, `docs/desktop.md` + `actions.rs`):

| Keys | Action |
|---|---|
| `Alt+Tab` / `Alt+Shift+Tab` | cycle focused window MRU forward/back (switcher overlay) |
| `Alt+1..4` | launch/focus primary apps |
| `Alt+F4` | close focused app |
| `Ctrl+Space` | command palette |
| `Alt+N` | notification history |
| `Ctrl+Alt+V` | clipboard history |
| `Ctrl+Alt+1..4` | switch workspace |
| `Ctrl+Alt+Shift+1..4` | move focused app to workspace |

QEMU/toolchain environment variables (`support/xtask/src/run.rs`,
`bundle.rs`, `main.rs`):

| Variable | Effect |
|---|---|
| `QEMU_HEADLESS=1/true/yes` | `-display none`; CI boots stay deterministic |
| `SERVICEOS_SMP=<n>` | guest CPU count (default 1; keeps boot output byte-stable) |
| `SERVICEOS_AUDIO=1` | attach virtio-sound-pci playback (host audiodev defaults to silent `none`) |
| `SERVICEOS_GL=1` | GTK display with `gl=on` instead of `gl=off` |
| `SERVICEOS_BOOT_MODE=full|reduced|safe|recovery` | compile the selected boot-mode word into every graph-capable loader; invalid values fail in host tooling and `xtask recover` selects `recovery` |
| `QEMU_ACCEL=kvm/tcg` | force accelerator (auto-detect `/dev/kvm` otherwise) |
| `QEMU_EXTRA_ARGS="..."` | appended verbatim to the QEMU command line |
| `QEMU_AUDIODEV=<spec>/off` | host audiodev spec override |
| `OVMF_CODE` / `OVMF_VARS` / `SERVICEOS_OVMF_VARS` | firmware binary overrides; throwaway vars copy location |

Terminal-service remote links (not env vars): `REMOTE_LISTENER_PORT`,
`REMOTE_AUTH_TOKEN`, `REMOTE_FRAME_MAX`, `RemoteBridge` — TCP remote operator
links with length-prefixed frames and first-frame auth token.

## 6. Testing & Tooling

xtask targets (`cargo xtask <cmd> --platform <plat> [--release]`;
platforms: `qemu-virtio`, `raspi5`, `virt`, `qemu-isa`, `riscv64-virt`):

| Command | What it does |
|---|---|
| `build` | arch crate + platform crate + image + userspace catalog per platform |
| `image` | stage bootable artifact (raw disk for virtio; Pi boot partition; etc.) |
| `run` / `qemu` | build, image, and launch QEMU with the env-var surface above |
| `recover` | convenience wrapper that runs with `SERVICEOS_BOOT_MODE=recovery` |
| `release` | release images for all platforms + `RELEASE-MANIFEST.json` |
| `test-upgrade` | boot-upgrade-boot cycle on qemu-virtio verifying persistence markers |
| `validate` | workspace check + tests + bounded qemu-virtio boot + optional aarch64 `virt` boot, grepping selftest markers (`selftest file-written bytes=`, `net-selftest end`, `selftest mix`) with a summary table |
| `ci-matrix` | CI matrix emission |

Host unit tests (`#[test]`) total ≈840 across the workspace: kernel 100,
shared 43, arch 5, userspace 692 (`find -name '*.rs' | grep -c '#\[test\]'`).
They cover, among others: scheduler stealing/preemption matrices, memory
pressure thresholds + OOM victim selection, loader relocation golden fixtures,
firewall rule-match matrix, resolver TTL expiry, ring wraparound math,
fence/ordering math, fsck/index consistency, wizard step sequencing, and
protocol codecs. On-target selftests: raspi5 bootstrap "device selftests",
storage-service boot read/write selftest, network selftest, audio mix selftest
(all asserted by `validate`'s boot-log greps). CI guidance in
`tests/README.md`: fmt/check/build plus bounded QEMU boots.

## 7. Known Limitations & Deferred Work

Honest, current gaps (details in `docs/roadmap.md` unless noted):

- **qemu-isa userspace entry**: kernel init succeeds but the first
  userspace-context restore faults (#GP, stale BIOS-era bytes); hypotheses and
  debug breadcrumbs documented in `docs/handoff-qemu-isa.md`
- **Network TX ring**: live boots negotiate the zero-copy TX ring but a
  kernel/service cross-view visibility gap trips the stall watchdog, reverting
  outbound traffic to the copied path until root-caused (roadmap §7)
- **OOM charging**: victim selection and hooks exist, but no spawn/map call
  site charges frames yet, so live footprints read zero until adoption
- **Dynamic linking remainder**: no PT_INTERP/ld.so-style external
  interpreters, lazy PLT binding, relro, or packaged split main+library
  binaries; runtime-service exec classifier still accepts flat + static
  ET_EXEC only (roadmap §11)
- **Graphics hardware**: single real display output (no second scanout);
  virtual mirror only; display mode-set is an honest contract stub — listed-
  but-unswitchable modes return Busy; GPU acceleration is out of scope (host
  `SERVICEOS_GL` only affects the QEMU window)
- **Windows/foreign ABI**: Linux-x86_64 syscall-number *translation skeleton*
  exists in runtime-service but nothing dispatches through it (everything
  unmapped is ENOSYS by contract); Windows runtime support entirely open
- **Crypto depth**: package feed signatures use the keystore verdict model,
  but account KDF and backup blob integrity are FNV-based non-cryptographic
  placeholders; real crypto signing/HKDF-grade derivation deferred
- **Audio/media**: null-sink mixing + PC-speaker/virtio-sound backends only;
  no real codec hardware path
- **Mode-set & power**: no real display mode switching; S3 suspend is a
  broadcast stub (not reliably exercisable under QEMU TCG); battery reporting
  probes ACPI-less paths and reports graceful absence
- **Networking breadth**: no IPv6 datagram plumbing (record parsing only),
  mDNS subset without multicast group operation/conflict resolution, firewall
  tags not yet promoted into `shared/abi`
- **Platform follow-ons**: raspi5 lacks framebuffer/graphical input/network/
  writable boot-store/audio backends (serial-first only, roadmap §6);
  riscv64 remains a parked skeleton
- **Workspace churn**: some concurrent-session refactor areas carry LSP noise
  at snapshot time (`handoff-qemu-isa.md` notes cosmetic host-target
  diagnostics); `cargo check --workspace` remains the source of truth
