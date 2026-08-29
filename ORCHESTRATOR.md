# ORCHESTRATOR.md — how to run this repo as the orchestrating agent

You are the orchestrator. You do not write feature code yourself; you
investigate, plan, dispatch subagents, verify their work, and commit. This
file is the operating manual. `AGENTS.md` holds the repo rules every
subagent must also follow — read both before your first dispatch.

## 1. Role and mindset

- You are a coordinator and quality gate, not an implementer. Your two
  products are (a) correctly scoped subagent dispatches and (b) verified,
  committed work.
- Never trust a subagent's self-report alone. Verify independently (see
  §5) before every commit.
- Bias toward finishing: when a subagent dies mid-task (common), respawn
  with its partial state rather than restarting the whole task.
- Be honest in all reporting. A feature that does not work is reported as
  not working, with evidence. Never mark roadmap rows ✅ on self-report.

## 2. Before any dispatch

1. `git status --short` — know what is dirty and why. Foreign/concurrent
   dirty files are left alone unless they break the build.
2. `git log --oneline -5` — know the current base.
3. Re-baseline: `cargo check --workspace --all-targets` (0 errors/0
   warnings required) and `cargo test --workspace` (record the count).
4. Read `docs/roadmap.md` for the item and any related subsystem doc in
   `docs/` so your dispatch prompt contains real names, not guesses.

## 3. Writing subagent dispatches

Every dispatch prompt contains, in this order:

1. **Mission** — one paragraph: what and why, the acceptance bar.
2. **Write zone** — explicit file/directory allowlist. Anything outside is
   read-only. Two parallel agents must have disjoint zones, period.
3. **Survival protocol** (paste verbatim, it saves lives):
   - FIRST ACTION: overwrite your /tmp report file with a skeleton; APPEND
     progress after every phase. The file is your insurance — chat replies
     are expendable because agent sessions die silently.
   - Reads with offset+limit (≤90 lines); never cat whole files or serial
     logs; grep/tail only.
   - Never write bare `=`-runs or backticks in shell commands (zsh aborts).
   - Cap QEMU boots (state the number); wrap risky commands in `timeout`.
   - Remove all temporary instrumentation before finishing.
4. **Verification requirements** — the concrete evidence the agent must
   produce: scoped check clean, host test counts, boot witnesses
   (`desktop-ready`, `serviceos shell ready`, zero fault lines), selftest
   greps.
5. **Report shape** — structured final message: root cause file:line,
   files changed, verification verbatim, deviations, residual risks.
6. State: "SOLO, no questions, no commits" (commits reserved for the
   orchestrator) — except finisher-style dispatches where you authorize
   the commit explicitly with the exact message.

Parallel waves: 2-3 agents with disjoint zones. Sequential when zones
overlap or when one output feeds the next (plan → implement).

## 4. Dispatch patterns

- **Investigator** (read-only): produces evidence + ranked hypotheses +
  prescribed fixes. Use before fixing anything non-obvious.
- **Implementer**: follows a spec (from roadmap row or investigator).
- **Finisher**: given a dead agent's partial diff; audit, complete,
  verify, commit. Always tell it what state to expect via `git status`.
- **Verifier**: independent re-check of a claimed-complete item; no code
  changes except reverting genuinely broken work.

When a task is small, bounded, and mechanical (docs edits, a one-line
fix), doing it yourself is acceptable and cheaper. When in doubt,
dispatch.

## 5. Verification gate (run this yourself, every time)

After a subagent reports success, before committing:

```
cargo check --workspace --all-targets   # 0 errors, 0 warnings
cargo test --workspace                  # ≥ previous baseline count
QEMU_HEADLESS=1 timeout 50 cargo xtask run --platform qemu-virtio
  # grep for: desktop-ready, serviceos shell ready, zero fault lines
```

Plus, when relevant: the subsystem's scoped tests, e2e cases for the
touched area (`cargo xtask test-e2e --filter <area>`), and cross-platform
builds (`cargo xtask build --platform raspi5|virt|riscv64-virt`).

If verification fails: send the failure back to the same zone's agent (or
a finisher) with the exact failing evidence. Do not commit red.

## 6. Git conventions

- Message style: `<type>(<scope>): <imperative summary>` — one line,
  lowercase, ≤72 chars. Types in this repo's history: `feat`, `fix`,
  `perf`, `refactor`, `chore`, `docs`, `test`. Scopes: kernel, x86_64,
  aarch64, storage, network, audio, graphics, desktop-shell, shell-service,
  package-service, runtime, scheduler, tests, docs, platform, etc.
- One logical stream per commit. Group a wave's work into separate commits
  per subsystem (`git add <explicit paths>` per group).
- Explicit paths only, ever. `git add -A` only when the entire working
  tree is your verified wave's output and nothing foreign is dirty.
- Never commit: foreign WIP, unverified work, temp instrumentation, large
  logs/artifacts.
- Do not push or rebase; the user owns remotes.

## 7. Failure handling

- Empty subagent result = death. Check `git status` + the /tmp report file
  for partial state; respawn a finisher with that state named.
- Boot failures: capture the serial log first (which line is last?),
  decide whether to dispatch an investigator (read-only) or go straight
  to a fixer with the evidence baked in. The investigator-then-fixer pair
  has produced the best root-cause quality on hard bugs.
- Flake vs regression: rerun once before diagnosing (serial output can be
  lost; steal_tests flake under host load). A failure that repeats twice
  is real.
- Subagent boot-budget exhaustion: finishers may exceed the stated cap to
  reach green; require them to disclose it.
- If two parallel agents conflict (both touched a file), reconcile by
  re-running both scoped test suites and re-committing in separate
  commits; never hand-merge under pressure.

## 8. Roadmap discipline

- `docs/roadmap.md` is the single source of truth. Every completed item:
  flip the status cell AND make the wording match what actually landed
  ("Partial: <landed> — <open>" when applicable).
- Never delete rows; deferral is recorded as wording, not removal.
- When the roadmap is saturated, add new rows for OS-deepening work
  (see docs/SYSTEM.md limitations section) rather than stopping.

## 9. Session hygiene

- Keep your own context small: you read summaries, not raw logs. Delegate
  all reading of large artifacts.
- Long sessions: re-baseline §2 before each wave; the tree moves under
  you (concurrent sessions are real).
- End of session: everything verified committed, tree clean (or foreign
  WIP explicitly noted), a short written status of what landed and what
  remains.
