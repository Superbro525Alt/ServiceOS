# AGENTS.md — ServiceOS

Guidance for AI agents working in this repository.

## Build / run / test

- workspace builds: `cargo check --workspace --all-targets` (must stay 0 errors / 0 warnings)
- unit tests: `cargo test --workspace` (counts drift upward; baseline ~213+)
- boot (headless): `QEMU_HEADLESS=1 cargo xtask run --platform qemu-virtio`
- other platforms: `--platform virt` (aarch64), `--platform qemu-isa` (BIOS/PVH),
  `--platform raspi5` / `--platform riscv64-virt` (build or banner tier)

## E2E test suite

The declarative e2e suite lives in `tests/` and boots REAL OS images under
QEMU. Use it to verify changes actually boot and behave.

- run everything: `cargo xtask test-e2e`
- target: `--platform qemu-virtio|virt|qemu-isa|riscv64-virt|raspi5`,
  `--tier 1|2|3|4`, `--filter <substring>`, `--tag <tag>`, `-j N` slots,
  `--report <file>` (TAP)
- exit codes: 0 = all pass, 1 = failures, 2 = harness error
- add a test: drop a TOML under `tests/cases/<tier-dir>/` — fields:
  `name`, `tier`, `platforms`, `witnesses` (serial lines to await),
  `fail_on` (fail-fast strings), `timeout_secs`, optional `tags`/`env`
- in-guest witnesses: services can emit `E2E <group>.<name> PASS|FAIL`
  behind `SERVICEOS_E2E_*` env gates (see graphics/desktop probes) —
  default boots must stay byte-identical when gates are unset
- a failing/wedged case: the runner kills QEMU at `timeout_secs` and marks
  the row Failed; debug from the printed serial tail, or re-run the single
  case via `--filter`
- full spec: `docs/test-plan.md` (frozen harness API) and `tests/README.md`

## Conventions

- commands may be wrapped by the `rtk` tool (`rtk cargo ...`, `rtk read`,
  `rtk grep`); plain commands also work
- never commit unrelated dirty files; use explicit paths
- kernel/core and arch/* are shared territory: keep diffs surgical, match
  existing style, no new dependencies without strong justification
