# ServiceOS E2E Test Plan

> **For agentic workers:** This is a design + work-package plan. Follow each WP
> verbatim; WPs 1–4 are independent (WP2/WP3 depend only on WP1's public API,
> specified exactly below). Append-only output contract: every WP ends with
> `cargo xtask test-e2e --tier <tier>` green on its own scope.

**Goal:** an automated end-to-end suite that boots the OS per platform and runs
live interaction tests so regressions are caught at service-graph, subsystem,
and scenario level — not just host unit tests.

**Architecture:** a new `tests/` crate workspace member hosts case definitions +
a shared runner lib (`tests/framework/`). Runner reuses xtask's QEMU command
builders and `bootlog.rs` serial-session driver, adding stdin injection (shell
scripting), per-test image isolation for parallelism, and witness-line matching.
A `test-e2e` subcommand in `support/xtask` orchestrates tiers, filters,
parallelism cap, and aggregation.

**Tech Stack:** Rust workspace (edition 2024), std-side runner crate (host),
existing no_std guest selftest hooks extended behind new `option_env!` gates.

## Global Constraints

- Guest builds are `no_std` with `panic = "abort"`; do not add std-dependent code to guest crates.
- All guest test gating uses compile-time `option_env!("SERVICEOS_E2E_*")` (matches existing pattern of `SERVICEOS_FARM_SELFTEST`, `REMOTE_LOOPBACK_SELFTEST` in `userspace/programs/developer-service/src/farm_harness.rs:42` and `userspace/programs/terminal-service/src/state.rs`); there is no runtime env passing into the guest.
- Default boots must stay byte-compatible: `serviceos-boot-mode=factory` runs must emit zero new lines from gated probes. This is asserted by a T1 case.
- Never mutate the dev images under `target/<platform>-image/`; all test boots stage copies.
- Host side is normal Rust std — unit-test the framework itself with `#[test]`.
- Commit style: conventional commits; each WP lands independently.

---

## 1. Test Taxonomy / Tiers

| Tier | Name | What | How | Platforms |
|---|---|---|---|---|
| T0 | compile/unit | Existing 185 workspace tests (`cargo test --workspace`) | unchanged | host |
| T1 | boot-smoke | Kernel boots to full userspace graph; banner witnesses per platform | serial grep (existing `bootlog::BootOutcome`) | qemu-virtio, qemu-isa, virt, riscv64-virt |
| T2 | service-graph bring-up | Storage/net/audio selftest markers present on factory boot; desktop-ready & shell-ready witnesses; no FAIL markers | marker grep over one bounded boot | qemu-virtio, virt (qemu-isa if graph up) |
| T3 | subsystem live suites | Drive actual operations: storage ops via boot selftest extension, fsck, net DHCP/selftest/firewall, audio capture/playback, graphics present counters, shell sessions/jobs/pipelines, pkg install flows, sysupdate flow, setup-wizard first boot, backup roundtrip, farm harness loopback, remote terminal loopback | in-guest selftest runners emitting `E2E <case> PASS/FAIL` + host-driven serial shell scripts | qemu-virtio primary, virt where supported |
| T4 | scenario/regression | Named tests for previously fixed bugs, each pinned to the fix's mechanism | same harness as T3, one case per regression | qemu-virtio (+virt) |

### T1 detail (banner tiers)

Each platform gets a declared smoke case in `tests/cases/smoke/`. Witness = last
line the platform reliably emits today (verified by current boot greps):

- `qemu-virtio`: storage+net+audio selftest markers (`MARKER_STORAGE`/`MARKER_NET`/`MARKER_AUDIO` in `support/xtask/src/validate.rs:11-13`) — full graph reachable.
- `qemu-isa`: kernel multiboot entry + init banner (same tier as validate's virtual boot but with `-machine pc -kernel ELF` path, see `run_qemu_isa` in `support/xtask/src/run.rs:102`). Tier here is kernel-entry banner only unless T2 probes are compiled in; declare `graph: minimal`.
- `virt`: same three selftest markers as qemu-virtio via `bounded_qemu_virt_boot` (`bootlog.rs:65`) — full graph incl. dhcp/slirp.
- `riscv64-virt`: OpenSBI handoff + kernel banner (`run_qemu_riscv_virt`, `run.rs:149`); build-only plus banner.
- `raspi5`: **build-only** case (RunKind::ManualDeploy) — asserts `xtask image --platform raspi5` produces staged dir with `config.txt`, `kernel8.img`, `serviceos/bootstore.bin` (see template text in `support/xtask/src/image.rs:233`).

### T4 named cases (initial set)

Each maps to a real commit/mechanism found in-tree:

| Case id | Regression source | Mechanism under test | Witness strategy |
|---|---|---|---|
| `regress.cursor-band-flush` | dd7d1f3 "keep cursor layer visible under present optimization" | cursor layer survives partial present/band flush in desktop-shell compositor | in-guest T4 probe drives N synth present cycles then asserts cursor-surface dirty accounting (new probe, WP3) |
| `regress.ghost-outlines` | window shadow code (`desktop-shell-service/src/windows/shadow.rs`) | stale shadow/border outlines cleared on move/close | probe issues hide/move cycle, asserts no ghost-dirty rects remain |
| `regress.dhcp-rx-delivery` | 473537d "post rx buffers eagerly and deliver inbound frames" | slirp DHCP offer reaches network-service through shared RX ring | existing DHCP lease log line in `network-service/src/main.rs:378` region; assert `NetworkAddressConfigured` timeline event within timeout |
| `regress.input-wakeup-race` | scheduler.rs:394-411 lost-wakeup guard (`raced_wakeup` latch clone) | input wake not lost when racing timer re-arm | kernel-level: run 512 synthetic idle/wake toggles from the E2E probe, assert zero dropped events counter |
| `regress.preempted-user-progress` | scheduler.rs:767 "interrupted user thread must be preempted immediately" | preemption of interrupted thread under load | background-spinner task + interactive latency sample in probe; assert progress increments while preempted peer spins |
| `regress.wizard-first-boot-chain` | f71686b "account reachability and non-blocking first boot"; root-manager/src/graph.rs:528 clean-exit chain | wizard completes → account-service reachable → marker persists → next boot skips wizard | two boots on same data volume (pattern reused from `upgrade.rs`): boot A asserts "setup: admin account created", boot B asserts wizard skip line |

New regressions get added as one file + one TOML case; see §2.

---

## 2. Harness Design

### 2.1 Repository layout (all new files)

```
tests/
  README.md                      # how to add a test (≤60 lines)
  framework/                     # host-side runner library crate "serviceos-e2e"
    Cargo.toml                   # deps: nothing external beyond std; reuse xtask helpers via extraction, see §2.3
    src/
      lib.rs                     # crate root, re-exports
      session.rs                 # SerialSession: spawn QEMU, piped stdout/stdin, line pump
      witness.rs                 # Witness matchers, TEST-line protocol parsing
      qemu.rs                    # QemuSpec { binary, args }, env overrides, isolation helpers
      isolation.rs               # staging dirs, data-image provisioning, OVMF vars overlay
      case.rs                    # CaseDef loading (TOML + directive fields)
      report.rs                  # CaseResult, TAP-ish aggregation, exit codes
      script.rs                  # SerialScript: scripted console interaction
  cases/
    smoke/virtio-smoke.toml ... isa-smoke.toml, virt-smoke.toml, riscv-banner.toml, raspi5-image.toml
    bringup/graph-factory.toml   # T2
    live/*.toml                  # T3: storage-ops, fsck, net-suite, audio-suite, graphics-counter,
                                 #        shell-jobs, shell-pipeline, pkg-install, sysupdate-flow,
                                 #        wizard-firstboot, backup-roundtrip, farm-loopback, remote-loopback
    regress/*.toml               # T4 one file per row in §1 table
    manifest.toml                # optional tags/prereqs metadata (kept tiny)
```

A crate at `tests/framework/` joins the workspace members list in the root
`Cargo.toml`. It compiles for the *host* only.

### 2.2 Test declaration format

TOML case files + shared runner lib (recommended). Rationale: adding a test =
one TOML file (+ optionally one guest probe). Rust integration tests inside
`tests/framework/tests/` would couple discovery to cargo test naming and make
parallel caps/filters awkward; a runner owns scheduling instead.

Canonical case file:

```toml
# tests/cases/regress/dhcp-rx-delivery.toml
[case]
name = "regress.dhcp-rx-delivery"
tier = 4
platforms = ["qemu-virtio", "virt"]
timeout_secs = 180          # overrides SERVICEOS_BOOT_TIMEOUT_SECS default for this case
witnesses = [               # host-side greps on combined serial output
  "net-selftest end ok",
  "E2E net.address-configured PASS",
]
fail_on = ["FAILED", "E2E net.address-configured FAIL"]
env_build = ["SERVICEOS_E2E_NET=1"]     # passed as option_env!-style env to guest builds
probes = []                              # guest-side named probes to run first; empty => pure witness
serial_script = ""                       # path relative to case dir; empty => no typing
data_fresh = true                        # provision zeroed data img per run (see §2.5)
tags = ["network"]
```

Semantics:
- `witnesses` all must appear before timeout ⇒ otherwise fail with tail dump.
- `fail_on` short-circuits to FAIL the moment any substring appears.
- `serial_script` routes the case through §2.4 scripting after the ready-witness line.
- Case names must be unique across all dirs; loader errors on duplicates.

### 2.3 Code placement rule (avoid duplicate QEMU logic)

`support/xtask/src/run.rs` already contains the canonical builders and they are
marked shared ("Shared by the interactive runner and the bounded headless
boot logger"). Do NOT copy them into tests/framework. Instead:

- Make `support/xtask/src/run.rs::{qemu_virtio_command,qemu_virt_command}` and
  the finder fns public-in-workspace by moving them (no logic change) into a
  new lib target `support/xtask-core` (created by WP1) OR simpler: publish them
  as `pub(crate)`→`pub` from xtask and give tests/framework a dependency on
  `xtask` itself (`support/xtask/Cargo.toml`). WP1 implementer picks whichever
  keeps `cargo xtask` behavior identical; acceptance criterion: zero diff in
  emitted QEMU argv between old and new paths for all five platforms.
- `tests/framework/src/qemu.rs` wraps those builders with an extra env layer:
  unique `SERVICEOS_OVMF_VARS` per concurrent qemu-virtio instance (mechanism
  already exists in `run.rs:453-459 create_ovmf_vars_copy`), unique MAC suffix,
  and `-serial stdio` (already default).
- Extend `bootlog.rs`'s `run_bounded` pattern into `session.rs`'s
  `SerialSession` that additionally: (a) accepts a timeout callback per
  witness-set progress (no-output watchdog, separate from total budget);
  (b) keeps stdin open for injection; (c) captures tail (default last 400
  lines) for diagnostics.

### 2.4 Serial-session driver & script format

```rust
// tests/framework/src/session.rs (API surface WP2/WP3 rely on)
pub struct SerialSession { /* child, reader thread, ring buffer of lines, tail */ }
impl SerialSession {
    pub fn spawn(spec: QemuSpec) -> Result<Self, Error>;
    pub fn wait_witness(&mut self, needle: &str, deadline: Instant) -> Result<(), Error>;
    pub fn send_line(&mut self, line: &str) -> Result<(), Error>;       // writes "\n"
    pub fn send_bytes(&mut self, bytes: &[u8]) -> Result<(), Error>;    // e.g. b"\x03" Ctrl-C
    pub fn wait_prompt(&mut self, deadline: Instant) -> Result<(), Error>; // matches shell prompt prefix
    pub fn tail(&self, n: usize) -> String;
}
```

Console path is line-oriented already: `shell-service` opens a console-session
over the raw serial path (`docs/shell.md`, `console_session_write/read-line`),
and `console-service` turns byte 0x03 into interrupt of pending read
(`shell-service/src/commands/diagnostics.rs`, "Ctrl-C 0x03"). So typed stdin
reaches the real operator shell — this is what makes T3 live interaction
possible without a GUI driver.

Script file format (referenced by `serial_script`):

```text
# tests/cases/live/scripts/pkg-install.txt
# each block: expect regex (deadline inherits case timeout proportionally)
expect: prompt
send: status health
expect: system health @tick
send: help
expect: exit code 0|^commands:
```

Directives implemented as plain regex-on-last-lines checks by `script.rs`; keep
it dumb (grep + type), not a screen-scraping UI driver.

### 2.5 Isolation & parallelism strategy

- Per-case stage dir `target/e2e/<case-name>/<slot>/` containing:
  - `serviceos.img` — copy of freshly built platform image (`create_platform_image`
    output copied once per scheduler batch, hardlinked/copied per slot).
  - `serviceos-data.img` — zeroed 128 MiB fresh file when `data_fresh = true`;
    copied seed when false (wizard/upgrade cases need cross-boot persistence).
  - qemu-virtio only: `OVMF_VARS-<slot>.fd` thrown-away copy via
    `SERVICEOS_OVMF_VARS` (prevents the poisoned-vars hazard documented in
    `bootlog.rs:52-54`).
- **Locks:** never point two instances at the same `.img`. The builder already
  stages copies (`validate.rs:196-203`, `upgrade.rs:14-33` follow this exact
  pattern); generalize it as `isolation.rs::stage_case_images(...)`.
- **Concurrency:** scheduler runs at most `j` QEMU processes (default min(4,
  free-mem-guess)). RAM budget: virtio ~1 GiB, virt 1 GiB (tcg multi), isa 1
  GiB, riscv 128 MiB. Cap defaults: x86-only batches 3–4 parallel, mixed
  arch 2. Overridable: `--jobs/-j`. Honor `QEMU_ACCEL` auto-kvm/tcg
  (`run.rs qemu_accel_mode`); KVM slots count against a stricter cap (2)
  because of memory bandwidth.
- Guest CPU determinism knobs preserved: pass through `SERVICEOS_SMP`,
  never enable `SERVICEOS_AUDIO=1` except the audio live case which sets it
  together with `QEMU_AUDIODEV=driver=none,id=speaker`.

### 2.6 Witness conventions (hybrid — recommended split)

Two layers:

1. **Host-side greps for existing boot evidence** (storage/net/audio markers,
   banner lines, DHCP lease lines, `"setup:"` lines). Zero guest changes, used
   in T1/T2 and cheap parts of T3/T4.
2. **Guest-emitted protocol lines for anything needing control flow**, from a
   single convention added by WP3:

```text
E2E <group>.<name> START
E2E <group>.<name> PASS [detail...]
E2E <group>.<name> FAIL [detail...]
E2E SUITE DONE pass=<n> fail=<m>
```

Rules: emitted by the in-guest runner behind `option_env!("SERVICEOS_E2E")`;
lines always logged via the standard `logf` path so they land on serial
untouched; `SUITE DONE` lets the runner kill early AND provides the second
signal (marker seen + counts consistent). Host matcher accepts either
full completion or per-witness satisfaction (declare `mode = "suite"` vs
`mode = "witness"` in the case TOML).

---

## 3. Guest-Side Probe Inventory

### Already runnable today (no guest changes needed)

| Probe | Location | Gate / Trigger | Witness |
|---|---|---|---|
| storage boot selftest (mount add, capability gate, blob write, persist, restore) | `userspace/programs/storage-service/src/selftest.rs` `run_boot_selftest` (called from `main.rs:150`) | always on | `selftest mount-added ok`, `selftest file-written bytes= N ok`, `selftest persist FAILED`, `selftest mount-present restored=1` |
| fsck scan/repair request handler | `userspace/programs/storage-service/src/fsck.rs:665 handle_fsck_request` | IPC (needs caller) | reply tag FsckReport — needs §new-shell hook or storage T3 probe |
| network UDP echo + TCP loopback selftest | `userspace/programs/network-service/src/protocol/selftest.rs` | always on | `net-selftest begin/end`, `net-selftest udp ... sent got replied echoed`, tcp estab/fwd/rep, ext pushed/drv_rx/drv_drop |
| DHCP vs slirp | `network-service/src/main.rs:176-267,378` | always on | lease/configured logs; `NetworkAddressConfigured` event |
| audio mix + capture selftests | `audio-service/src/service.rs:61-81` | always on | `selftest mix frames clip ...`, `selftest capture reads frames sum tick` |
| developer farm harness | `developer-service/src/farm_harness.rs` | `SERVICEOS_FARM_SELFTEST=1` (build-time) | `farm-selftest PASS job=HARNESS_JOB_ID target=...`, port 44210, tags 0xd22/0xd23, delayed 2500 turns |
| remote terminal loopback | `terminal-service/src/remote.rs selftest_loopback` | `state::REMOTE_LOOPBACK_SELFTEST` + listener up, fires at remote_turns ≥ 500 (`main.rs:85-88`) | loopback success log line |
| setup-wizard first boot | `setup-wizard/src/main.rs`; chain mgmt in `root-manager/src/graph.rs:528` | first-boot detection | `first boot detected; starting setup`, `setup: admin account created`, marker `state/setup-wizard/firstboot.done` persisted |
| backup snapshot/restore engine | `backup-service/src/main.rs` (`handle_restore`, `plan_restore`, `RestoreReport`) | IPC | needs invocation path — see shells |

### New tiny probes required (WP2 scopes each precisely)

All follow the existing `option_env!` gate pattern; name prefix reserved: `SERVICEOS_E2E_…`.

1. **E2E suite runner skeleton** in a shared runtime location used by services
   with selftests (`userspace/programs/storage-service`, …). Minimal: a small
   helper in `userspace/programs/runtime` exporting `e2e_log_pass/fail(name)`
   + an ordinal runner that walks registered probe list and prints the §2.6
   protocol. Guest crates add entries gated on their own flag.
2. **`SERVICEOS_E2E_STORAGE=1` deep-storage probe** — extends
   `selftest.rs::run_boot_selftest` (or sibling module `selftest_e2e.rs`) to
   cover: directory ops, unmount/re-mount capability rejection
   (`root.rs try_unmount`), persistent store roundtrip beyond boot selftest,
   fsck with corruption seeded (`fsck.rs` apply=true repair), index query
   (`state.rs`/`index.rs` naming). Emits E2E lines.
3. **`SERVICEOS_E2E_SHELL=1` shell probe** — when enabled, `shell-service`
   registers an internal `e2e` builtin command executing canned sub-cases
   locally: jobs spawn/wait (`shell-service/src/jobs.rs`), pipeline fanout
   (`pipeline.rs`), history search (`history_search.rs`), deny rules lookup —
   emitting E2E lines. Complements host-typed serial scripts: use the built-in
   probe for non-deterministic timing pieces.
4. **Input-event counters export** — peripheral-service/desktop-shell debug
   counter behind `SERVICEOS_E2E_INPUT=1`: totals of reported pointer/key
   events serviced after the lost-wakeup guard (`kernel/core/src/task/scheduler.rs:401`)
   become assertion targets; wakes scheduler's wakeup-latch path N times and
   emits `E2E input.counters delivered=<n> lost=<n>`.
5. **Graphics present-counter export** — read the existing `present_count`
   stat fields from `graphics-service` (present_count tracked ~7 sites, grep
   present_count in that crate) and emit `E2E gfx.present outputs=<k>
   frames=<n> fences=<n>`; consumed by `live/graphics-counter` and
   `regress.cursor-band-flush` (drive partial-present cycles via synth dirty
   bands around the cursor surface, then assert cursor still visible in dirty
   accounting — the dd7d1f3 invariant).
6. **Guest-initiated exit witness** — prefer keeping kill-based termination;
   add nothing here UNLESS flakiness appears. If needed later: emit
   `E2E SUITE DONE` and attach `-device isa-debug-exit` in qemu.rs wrapper;
   guest pio write exits cleanly. Explicitly out of scope for WP1–WP4 v1.
7. **No scripted-session env var in guest**: host side suffices (`§2.4`);
   don't add a guest feature for it.

---

## 4. xtask Integration

### New subcommand: `test-e2e`

CLI (`support/xtask/src/cli.rs` gets `CommandKind::TestE2e`):

```
cargo xtask test-e2e [--platform <p>] [--tier 0..4] [--filter <substr-or-regex>]
                     [--tag <t>] [-j <n>] [--timeout-secs <s>] [--report <path>]
                     [--keep-all] [--release] [--list]
```

Behavior:
- Builds once per platform (reuse `build_for_platform` +
  `create_platform_image` from `build.rs`/`image.rs`), then schedules cases.
- Tier filter includes lower tiers implicitly (`--tier 3` ⇒ T0 skip note, T1+
  T2+T3 run); `--tier 4` means T4 only for iteration speed? No — tier 4 ⊃ T1–T3.
- `--platform` restricts matrix; cases listing other platforms skip (not fail).
- Missing emulator binaries ⇒ platform marked SKIPPED (mirrors
  `validate.rs:74-80` behavior); raspi5 image case always runs (host-side only).

Result reporting (`report.rs`):
- Human summary table like `validate.rs:82-93` (aligned columns, PASS/SKIP/FAIL).
- `--report <file>` writes TAP-ish stream plus a machine footer:

```tap
ok 12 - regress.dhcp-rx-delivery # elapsed 41.2s platform=virt
not ok 13 - regress.ghost-outlines # timeout; TAIL_START ... TAIL_END
1..13 # plan=pass=12 fail=1 skip=0 duration_wall=...
```

- Exit codes: 0 all pass/skip; 1 any FAIL; 2 infrastructure error (image build
  failure, QEMU spawn failure not attributable to the case).
- CI compatibility (`ci.rs` exists for ci-matrix today): future job =
  `cargo xtask test-e2e -j 4 --report target/e2e/report.tap` (single line,
  TAP parseable at `target/e2e/report.tap`); keep wall-clock bounded via
  `--timeout-secs` ceiling ≈ SERVICEOS_BOOT_TIMEOUT_SECS default (240).

Guest rebuild awareness: cases declaring `env_build` invalidate a cached guest
build keyed by flag tuple; store fingerprint in `target/e2e/build-fingerprint.json`.
(Keeps default-flagged builds fast.)

WP4 scheduling contract (as landed):
- `-j N` (default 1, hard cap 8 — higher values are refused with exit 2).
  Builds hydrate serially per platform+gate-tuple BEFORE any worker starts;
  boot rows then run on `N` worker threads, tier-ordered (fast smokes first,
  long TCG/high-tier last), with each row's per-case timeout enforced
  concurrently and a deterministic name-sorted summary/TAP regardless of
  completion order. TCG-platform rows additionally serialize against each
  other (one process-wide TCG execution slot inside `run_case`; KVM and
  no-emulator rows unaffected) as mitigation for the TCG host-wedge lottery.
- Per-tuple image snapshots live under `target/e2e/builds/<tuple>/` (the
  builder's fixed output path would otherwise be clobbered across tuples);
  slots stage copies from there, so no two boots ever share an image, and
  qemu-virtio keeps its throwaway per-slot OVMF vars overlay.
- PASSing rows prune their stage dirs; failures retain them (`--keep-all`
  disables pruning).
- No-output watchdog is a separate knob from the per-case budget:
  case `idle_timeout_secs` > env `SERVICEOS_IDLE_TIMEOUT_SECS` >
  min(180s, case budget).

---

## 5. Work Packages

### WP1 — Harness core + smoke tier (`tests/framework/*`, `support/xtask/test-e2e` wiring)

Files:
- Create: `tests/README.md`, `tests/framework/{Cargo.toml,src/lib.rs,src/session.rs,src/witness.rs,src/qemu.rs,src/isolation.rs,src/case.rs,src/report.rs}`,
  `tests/cases/smoke/{virtio-smoke.toml,isa-smoke.toml,virt-smoke.toml,riscv-banner.toml,raspi5-image.toml}`
- Modify: root `Cargo.toml` (workspace members += `tests/framework`), `support/xtask/src/cli.rs` (`TestE2e`), `support/xtask/src/main.rs` (dispatch), exposure of `qemu_virtio_command`/`qemu_virt_command` per §2.3.

Public API relied on by WP2–WP3 (freeze now):
```rust
e2e::CaseDef;            e2e::load_cases(root) -> Vec<CaseDef>;
e2e::SerialSession::{spawn,wait_witness,send_line,send_bytes,wait_prompt,tail};
e2e::run_case(&CaseDef, &RunCtx) -> CaseResult;   // RunCtx{stage_root, jobs, builds}
e2e::aggregate(vec<CaseResult>) -> ExitCode;      // codes per §4
```

Steps:
1. Extract/publish QEMU builders (verify identical argv diff — acceptance gate).
2. Implement SerialSession via refactor of `bootlog.rs::run_bounded` reading
   loop (keep bounded_qemu_* working; validate passes afterwards).
3. CaseDef TOML loader + validation errors with file:line context.
4. Scheduler: sequential first (j=1 correct), then semaphore j-cap; OOM-safe
   slot counting by per-platform memory spec table in `isolation.rs`.
5. Wire xtask flags; wire report writer.
6. Smoke cases using existing markers only.

Acceptance criteria:
- `cargo xtask test-e2e --tier 1` green locally on machines with
  qemu-system-x86_64/aarch64/riscv64 installed; SKIPPED rows otherwise; exit
  semantics per §4 verified by a framework unit test (inject fake outcomes).
- Two parallel `-j 2` virtio runs produce distinct stage dirs and both pass
  (single QEMU boot verification allowed here; it satisfies the ≤2-boots rule).
- `cargo xtask validate` and `cargo xtask test-upgrade` unchanged behavior.

### WP2 — Port existing selftests into declared cases + new storage probe

Files:
- Create: `tests/cases/bringup/graph-factory.toml`, `tests/cases/live/{storage.toml,network.toml,audio.toml,audio-virtio.toml,fsck.toml,index.toml,pkg.toml,sysupdate-history.toml}`,
  `tests/cases/live/scripts/{pkg-install.txt,sysupdate-history.txt}`. Case ids
  use `<subsystem>.live` naming; audio's SERVICEOS_AUDIO=1 virtio variant is
  its own tagged case (`audio.live.virtio`) per §2.5 gating.
- Create (harness gaps filled additively by WP2): `serial_script` execution
  hook in `run_case` (§2.4 semantics: every send anchored on a prior expect,
  remaining wall budget shared across expects, witnesses still gate success)
  and the `qemu_env` case key (launch-time env pairs applied under the
  existing EnvGuard window; needed for the audio variant's
  SERVICEOS_AUDIO/QEMU_AUDIODEV pair).
- Deferred to the T4/scenario work package of record: `farm-loopback`,
  `remote-loopback`, `wizard-firstboot` cases and the gated deep-storage /
  runtime `e2e.rs` helper probes (§3 items 1–2) — none are required for the
  always-on inventory rows above.
- Documented skip: `backup-roundtrip` — backup-service's snapshot/restore
  engine is reachable only via IPC (`backup-service/src/main.rs handle_restore`);
  there is no `backup` shell verb (`shell-service/src/commands/mod.rs`), so a
  serial script cannot drive it today. Revisit when a console path lands.

Acceptance criteria:
- Each listed case passes against an unmodified-fix tree.
- Factory-boot byte-compat proof: `cargo xtask run --platform qemu-virtio`
  without `SERVICEOS_E2E*` shows no `E2E ` prefixed lines (asserted by a
  dedicated negative case `smoke/no-probe-defaults.toml` shipped in this WP).
- Farm/remote-loopback cases compile with their historical env flags set via
  `env_build` and reproduce known-PASS evidence strings.

### WP3 — Shell/gfx/input/sysupdate/backup cases + T4 regression suite + E2E witness conventions in-guest

Files:
- Create: `tests/cases/live/{graphics-counter.toml,shell-jobs.toml,shell-pipeline.toml,pkg-install.toml,sysupdate-flow.toml,backup-roundtrip.toml,input-counters.toml}`,
  `tests/cases/regress/{cursor-band-flush.toml,ghost-outlines.toml,dhcp-rx-delivery.toml,input-wakeup-race.toml,preempted-user-progress.toml,wizard-first-boot-chain.toml}`
- Modify: `audio`, `graphics-service` (counter export, `SERVICEOS_E2E_GFX`),
  `peripheral-service`/`desktop-shell-service` (input counters, cursor/probe
  hooks, `SERVICEOS_E2E_INPUT`, ghost-outline assertions), `shell-service`
  (internal `e2e` builtin, `SERVICEOS_E2E_SHELL`), `package-service`/pkg shell
  commands pathway coverage via typed serial script
  (`tests/cases/live/scripts/pkg-install.txt`), backup snapshot→restore on
  private mount then verify bytes (round trip witness from `RestoreReport`).

Acceptance criteria:
- All six T4 rows from §1 exist, fail-to-pass verified for at least one case by
  temporarily reverting the associated mechanism (documented recipe in
  tests/README.md — e.g. revert-commit replay or localized comment-out) —
  that's the suite's teeth proof.
- Serial-scripted shell cases use only real command surface
  (`status`, `logs`, `service`, `pkg install/update/query`, sysupdate cmds);
  no invented commands.

### WP4 — Parallelism/isolation hardening + docs + CI

Files:
- Modify: `tests/framework/src/isolation.rs` (fingerprint cache, disk-space
  guard), `support/xtask/src/ci.rs` (add e2e lane), `docs/test-plan.md` link
  stub `tests/README.md`.
- Create: `.github/workflows` lane addition IF repo CI workflow expects lanes
  (check `ci.rs` ci-matrix consumer first; if none, document manual invocation).

Acceptance criteria:
- `cargo xtask test-e2e -j 4 --tier 2` completes with ≥3 concurrent QEMUs, no
  shared-file contention (run twice back-to-back without dev-image mutation:
  checksum `target/x86_64-*-image/serviceos.img` before/after equality).
- Full `-j default` suite wall time recorded in tests/README.md; TAP file valid
  (`prove` or simple parser test).
- No disk leak: stage dirs pruned for PASSing cases, retained for failures
  (configurable `--keep-all`).

Dependency order: WP1 → (WP2 ∥ WP3) → WP4. WP2 and WP3 share no files.

---

## 6. Risks / Unknowns

1. **TCG wall-clock blowup.** virt platform runs `tcg,thread=multi` (~10× slow
   vs KVM). Live suites on virt may exceed 240 s default; per-case
   `timeout_secs` + `-j` mixing guidance addresses, but initial timeout values
   in the TOML files are estimates — calibrate on landing WP2/3.
2. **Guest has no poweroff path** — every test terminates by QEMU kill. Kill
   mid-write could theoretically corrupt staged images; acceptable because
   every stage is disposable, but upgrade/wizard cases reuse data volumes
   across boots — ensure kills happen only after expected witness observed
   (`markers_seen` gate like `bootlog.rs` does today).
3. **Byte-stability of default boot serial output** is load-bearing for the
   "no new lines" gate and existing greps; any logging change upstream breaks
   many witnesses silently. Mitigation: T1 asserts both presence of markers
   AND stable sentinel ordering (first N well-known lines), making drift loud.
4. **Serial stdin reliability under slab buffers**: `send_line` before the
   shell prompt exists may drop input into firmware/kernel phases. Script
   engine always anchors on `prompt` expectation first. Resolved during WP2:
   the glyph is `serviceos> ` (`SHELL_PROMPT`, `shell-service/src/lib.rs`);
   console-service additionally drops bytes typed while no readline session is
   armed (`console-service/src/input.rs handle_input_byte`), so scripts must
   expect each command's reply before the next send.
5. **OVMF vars churn**: SERVICEOS_OVMF_VARS overlay exists; confirm template
   copy race-free when j>2 (create_ovmf_vars_copy is not atomic). Wrap copy in
   temp-name+rename inside WP1 isolation layer rather than relying on xtask's
   direct call when concurrency >1 (or serialize vars-template copies).
6. **Compile-time env gates multiply build configs** — fingerprint caching
   (WP4) required or `--release` + flag combos quadruple CI time. Acceptable
   initially: gates only in WP3-selected services.
7. **arm virt GPU/input devices** differ (virtio-*-device vs *-pci); input
   counters probe (§3.4) must not assume PCI. Verify peripheral-service device
   abstraction covers virtio-keyboard-device enumeration before shipping
   `input-counters.toml` on virt.
8. **raspi5/riscv64 tiers are intentionally shallow** (build/banner). Growing
   them requires UART-quality service graphs not yet present (raspi docs note
   deferred graphics/net/storage backends in `image.rs:233` README text).
   Keep those platforms opt-in via explicit `--platform raspi5` so suite time
   doesn't pay for unimplementable cases.
