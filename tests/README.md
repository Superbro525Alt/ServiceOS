# ServiceOS E2E Tests

Declarative end-to-end suite driven by `cargo xtask test-e2e`
(docs/test-plan.md is the frozen spec for tiers, harness API, and case
schema).

## Layout

```
cases/
  smoke/   T1 boot smokes: banner/marker witnesses per platform
  bringup/ T2 factory service-graph bring-up
  live/    T3 subsystem suites          (WP2+)
  regress/ T4 regression pins           (WP3+)
framework/ host-side runner crate (`serviceos-e2e`)
```

## Adding a test

1. Drop a TOML file under the right tier dir:

```toml
[case]
name = "regress.my-bug"            # unique across all dirs
tier = 4
platforms = ["qemu-virtio"]        # any of qemu-virtio|virt|qemu-isa|riscv64-virt|raspi5
witnesses = ["net-selftest end ok"]
fail_on = ["E2E my-bug FAIL"]      # optional short-circuit strings/patterns
tags = ["network"]                 # optional, enables --tag selection
```

Witness patterns are substring greps by default; `\d \s \w`, groups,
`|`, `^ $`, and quantifiers work too. Every case runs against slot-isolated
image copies under `target/e2e/<case>/slot0/` — dev images are never mutated.

2. If the case needs guest interaction beyond passive evidence, add a guest
probe behind an `option_env!("SERVICEOS_E2E_*")` gate (WP2+) and declare it
with `env_build`; a serial script goes into `<case-dir>/scripts/*.txt`.

3. Verify locally:

```
cargo xtask test-e2e --list               # discovery check
cargo xtask test-e2e --filter my-bug      # just your case
```

Exit codes: 0 pass/skip, 1 assertion failure, 2 infrastructure error.
Missing emulators surface as SKIP rows; builds happen once per platform.
Parallel `-j` scheduling and slot pruning land in WP4. T0 remains
`cargo test --workspace`.

Known quirk: `virt` boots use `-accel tcg,thread=multi` (~10× slower than
KVM) and can sit silent for minutes mid kernel selftests (plan §6.1);
calibrate its `timeout_secs` per host before tightening budgets.

Known host flake (TCG wedge lottery on multi-tenant hosts): a TCG guest
(`virt`, `riscv64-virt`, `qemu-isa`) intermittently wedges permanently —
symptom is a case timeout with zero/minimal guest progress or a network-rx
stall while tx continues. Tree-independent and nondeterministic; an isolated
rerun of the failing case passes. The runner serializes TCG cases (one
process-wide TCG slot) to reduce incidence; it cannot eliminate the wedge.
