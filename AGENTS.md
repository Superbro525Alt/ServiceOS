# AGENTS.md — ServiceOS

Guidance for AI agents working in this repository. Read fully before your
first action; it encodes hard-won lessons from many prior agent sessions.

Acting as the ORCHESTRATOR (dispatching subagents, verifying, committing)?
Read `ORCHESTRATOR.md` — it is the operating manual for that role.

## Current state snapshot (verify, don't trust)

- Rust capability-based microkernel-style OS: `kernel/core` + `arch/*` +
  `platform/*` + userspace services + desktop shell. See `docs/SYSTEM.md`
  for the full as-built architecture tour and `docs/roadmap.md` for status.
- Platforms: `qemu-virtio` (x86_64 UEFI/KVM, primary), `qemu-isa`
  (x86_64 BIOS/PVH), `virt` (aarch64 full userspace graph under TCG),
  `raspi5` (build tier), `riscv64-virt` (banner tier).
- Workspace unit tests: run `cargo test --workspace`; counts drift upward
  (baseline ~213, re-baseline yourself on first run). `cargo check
  --workspace --all-targets` must stay 0 errors / 0 warnings.
- E2E suite: `cargo xtask test-e2e` — 20+ live cases across all platforms,
  fully green as of last full run (see `tests/README.md`, `docs/test-plan.md`).

## Build / run / test

- headless boot: `QEMU_HEADLESS=1 cargo xtask run --platform qemu-virtio`
  (KVM auto-selected when `/dev/kvm` exists)
- boot evidence: grep serial output for `desktop-ready` and
  `serviceos shell ready`; zero `panic|triple|general protection` lines
- quick e2e: `cargo xtask test-e2e --platform qemu-virtio --filter smoke`
- full e2e (parallel, CI-ready):
  `cargo xtask test-e2e -j 4 --report target/e2e/report.tap`
- release + artifacts: `cargo xtask release` (manifest in
  `target/release/RELEASE-MANIFEST.json`); upgrade-matrix:
  `cargo xtask test-upgrade`; all-in-one gate: `cargo xtask validate`
- env knobs: `SERVICEOS_SMP=2` (second AP), `SERVICEOS_AUDIO=1`
  (virtio-sound + capture probes), `SERVICEOS_GL=1` (GTK GL display;
  default off — breaks input on some hosts), `SERVICEOS_BOOT_MODE=full|
  reduced|safe|recovery` (fresh data image boots the setup wizard;
  recovery = minimal storage+console+backup graph), `SERVICEOS_E2E_*`
  (in-guest witness gates), `SERVICEOS_FARM_SELFTEST=1`

## The e2e suite

Declarative TOML cases under `tests/cases/{smoke,bringup,live,regress}/`;
framework in `tests/framework/`; runner `cargo xtask test-e2e` with
`--platform --tier --filter --tag -j N --report`.

- case schema: `name`, `tier`, `platforms`, `witnesses` (serial lines to
  await), `fail_on` (fail-fast), `timeout_secs` (REQUIRED — a case without
  it inherits 240s and can stall the whole run), `idle_timeout_secs`,
  optional `tags`/`env`
- exit codes: 0 all pass / 1 failures / 2 harness error
- in-guest witnesses: services emit `E2E <group>.<name> PASS|FAIL` behind
  `SERVICEOS_E2E_*` gates; default boots must stay byte-identical when
  gates are unset
- frozen harness API + design rationale: `docs/test-plan.md`; case schema:
  `tests/README.md`

## Known gotchas (learned the hard way)

- **Image lock**: a still-running QEMU (yours or the user's) holds
  `target/images/.../serviceos.img` — `cargo xtask run` fails with
  `Failed to get "write" lock`. Kill stale QEMU or copy images aside; do
  not "fix" code for this.
- **Serial flakiness**: first headless boot after heavy parallel builds
  can lose early serial lines (banner missing) with no fault — rerun once
  before diagnosing.
- **TCG timing**: aarch64/TCG stretches legit silent stretches (DHCP
  discovery etc.); don't shrink global timeouts to chase speed — set
  per-case `timeout_secs` instead. ARM virt wall-clock ≈ 2x x86.
- **rtk wrapper**: commands may be wrapped (`rtk cargo ...`, `rtk read`,
  `rtk grep`, `rtk git ...`); plain commands also work. `rtk` truncates
  output — pipe through grep/tail rather than trusting full dumps. Never
  write bare `=`-runs or backticks into shell commands (zsh aborts).
- **Concurrent sessions**: multiple agent sessions may edit this repo at
  once. Always `git status` first; commit with explicit paths only; if a
  foreign file is dirty, leave it alone unless it breaks the build (then
  fix minimally and say so loudly in your report).
- **Known flake**: `kernel/core` steal_tests occasionally fail under heavy
  parallel host load — isolated rerun passes; do not "fix" the scheduler
  for this.
- **Long subagent sessions die silently**: structure your work so
  progress survives (write findings to a file continuously), keep single
  commands under ~100s, cap QEMU iterations, never cat full serial logs
  (grep/tail only).

## Conventions

- never commit unrelated dirty files; explicit `git add <paths>`
- kernel/core and arch/* are shared territory: surgical diffs, match
  existing style, no new dependencies without strong justification
- shared/abi edits are additive-by-default: extend reply words at the END,
  bump no layouts, keep old readers compiling
- userspace services follow the house pattern: own channel tags, additive
  status replies, `#[cfg_attr(not(test), no_std|no_main)]` for host
  testability, host tests via `CARGO_TARGET_DIR=/tmp/<x> cargo test -p
  <pkg>` from `userspace/programs`
- fix root causes, never symptoms; temporary instrumentation must be
  removed before finishing (grep for your prefixes)
- docs live in `docs/`; keep `docs/roadmap.md` wording honest — flip ✅
  only when the code proves it, prefer "Partial: <landed> — <open>" rows

## Where to look

- as-built architecture: `docs/SYSTEM.md` (start here)
- roadmap + honest status per area: `docs/roadmap.md`
- test harness spec: `docs/test-plan.md`
- per-subsystem docs: `docs/{memory,networking,storage,graphics,audio,
  packages,security,terminal,desktop,platforms,...}.md`
- known open platform bug notes: `docs/handoff-qemu-isa.md`
