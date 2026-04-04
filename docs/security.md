# Permissions, Trust, and Security UX

## Role

The current security UX layer makes the existing capability and trust model
reviewable and editable without pretending the UI is the enforcement layer.

The important split is:

- `root-manager` still enforces native image launch policy
- `runtime-service` still enforces hosted-environment approval state
- `package-service` still owns repository/package trust and provenance state
- `security-service` owns native app launch-policy review, override state, and
  audit history
- shell, settings, and software-center surfaces are clients of those real
  services

That keeps prompts and review UI honest instead of turning them into fake
cosmetic approval flows.

## Native app authority review

`security-service` owns the current native-app review/edit model.

It tracks a small explicit policy table for the launchable graphical/native
images that currently matter most to the desktop and operator workflow:

- `settings-app`
- `files-app`
- `monitor-app`
- `terminal-app`
- `software-center-app`
- `sysinfo-tool`
- `posix-host-tool`
- `cross-builder-tool`

Each entry surfaces:

- image identity
- the real capability group summary granted at launch time
- the sensitive subset of those capabilities
- a policy state:
  - `default-allow`
  - `allowed`
  - `blocked`

`root-manager` consults that policy before launching native images and returns
`permission-denied` when the policy blocks the request.

## Runtime approval state

`runtime-service` now exposes approval-aware environment state instead of
treating every hosted environment as equally trusted.

Current runtime environment states include:

- `ready`
- `busy`
- `destroyed`
- `pending-approval`
- `denied`

If a runtime profile requests sensitive hosted-workload capabilities,
`runtime-service` can hold the environment in `pending-approval` and block run
launches until the approval state changes. The current packaged `posix` profile
does not request those sensitive capabilities by default, so the approval path
is real but mostly quiet in the base image today.

## Trust and provenance surfaces

Package and repository trust stay rooted in `package-service`.

Current review surfaces include:

- package trust state
- package source provenance
- package channel/ring policy
- package rollback provenance
- repository trust mode
- repository sync state

The software center already exposes package trust metadata in the desktop, and
the shell now exposes the same state under `security repos` and
`security package <name>`.

This remains honest about current enforcement:

- boot repository trust is real
- digest-pinned and unsigned repository modes are visible
- there is not yet cryptographic repository feed signing, key rotation, or
  stronger trust-root management

## Desktop and operator surfaces

Current shell commands:

- `security apps`
- `security app <name> [allow|block|default]`
- `security runtimes`
- `security runtime <env-id> [approve|deny|reset]`
- `security repos`
- `security package <name>`
- `security workspace <id>`
- `security audit [count]`

Current desktop surface:

- `settings-app` now has a `SECURITY` page that reviews native app launch
  policy, sensitive capability groups, runtime approval state, and recent
  security audit entries

The desktop surface edits the same real policy state the shell sees. It does
not bypass manager or runtime enforcement.

## Audit and history

Current audit coverage is intentionally small but real.

- `security-service` records native launch-policy changes
- `runtime-service` records hosted-environment approval requests and approval
  changes
- both emit structured log events through the normal logging pipeline
- shell review uses the real audit data instead of a UI-local history

## Denial semantics

Current denial/reporting paths are:

- native app launch blocked by policy:
  - manager launch request returns `permission-denied`
  - `root-manager` emits `security-launch-denied`
- runtime launch blocked by approval state:
  - runtime launch returns `pending-approval` or `denied`
- package/repository trust mismatches:
  - continue to surface through the existing package-service status model and
    software-center/package shell flows

## Scope note

This phase does not claim a finished security architecture.

Still deferred:

- richer desktop prompt UX for runtime approvals
- stronger cryptographic signing and trust-root rotation
- broader developer-workspace trust policy editing
- stronger compatibility sandbox expansion
- wider desktop-facing denial and authority explanations across all apps

Open follow-on work is tracked centrally in [docs/roadmap.md](roadmap.md).
