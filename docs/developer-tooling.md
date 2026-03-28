# Developer Toolchain And Cross-Target Workflows

## Role

The developer workflow layer is split into three pieces:

- `package-service` distributes developer support packages and SDK metadata
- `developer-service` owns toolchain, workspace, and build-job policy
- `cross-builder-tool` is a transient worker launched by `developer-service`

That keeps package/update policy, long-lived developer state, and one-shot build
execution separate.

## Toolchain model

Toolchains are package-delivered descriptors, not hardcoded shell shortcuts.

The current `developer-service` package provides:

- `serviceos-native`
  - target: `native-x64`
  - state: `installed`
  - format: `serviceos-flat`
- `linux-x64`
  - target: `linux-x64`
  - state: `installed`
  - format: `elf64`
- `windows-x64`
  - target: `windows-x64`
  - state: `installed`
  - format: `pe32+`
- `macos-x64`
  - target: `macos-x64`
  - state: `remote-only`
  - format: `macho64`

The important point is that the public model already distinguishes:

- installed local toolchains
- unsupported targets
- future remote-only targets

That keeps macOS preparation honest without pretending local signing,
notarization, or full Apple SDK parity exists inside ServiceOS today.

## Workspace model

Workspaces are also package-delivered descriptors for now.

Each workspace declares:

- a stable workspace name
- an artifact stem
- a source payload path
- a target-to-toolchain mapping

The first packaged workspace is `hello-cross`. It proves the platform flow
without depending on writable project directories, which are still deferred to a
later storage phase.

## Build and invocation flow

The current build flow is:

1. the operator installs the `developer-service` package
2. `developer-service` loads its toolchain and workspace catalog from packaged
   resources
3. the shell asks `developer-service` to build a workspace for a selected target
4. `developer-service` opens only the declared source payload through
   `storage-service`
5. `developer-service` copies that source into a kernel memory object
6. `developer-service` asks `root-manager` to launch `cross-builder-tool`
7. `cross-builder-tool` receives only:
   - a text relay for build logs
   - a report channel back to `developer-service`
   - the source memory object
8. the worker emits an artifact into a new memory object and reports the result
9. `developer-service` records the job and exposes the artifact through its
   service contract

This is intentionally capability-scoped:

- the worker does not receive storage-root authority
- the shell does not spawn the worker directly
- the package system does not own build execution

## Target support

Current local target support is real for:

- `native-x64`
- `linux-x64`
- `windows-x64`

The current worker emits:

- a ServiceOS flat image artifact for native
- an ELF64 artifact for Linux
- a PE32+ artifact for Windows

Current macOS behavior is intentionally explicit:

- `macos-x64` appears in toolchain and workspace metadata
- it is marked `remote-only`
- local build requests fail with a clear “not locally supported yet” result

That preserves a clean future path for remote build/sign/notarization services
without lying about local support.

## Capability model

Developer tooling does not get ambient global power.

Current authority boundaries are:

- `shell-service`
  - lookup access to `developer-service`
- `developer-service`
  - startup grant to `log-service`
  - startup grant to `storage-service`
  - one packaged catalog resource blob
  - manager-mediated launch authority only for `cross-builder-tool`
- `cross-builder-tool`
  - no storage lookup authority
  - no package authority
  - no direct network authority
  - only the source memory object, report channel, and text relay transferred by
    `developer-service`

This is the important architectural line for later growth:

- toolchains are not ambient filesystem trees
- build workers are not ambient shells with global access
- project inputs are passed explicitly

## Terminal and desktop integration

The developer workflow currently integrates through the shared shell stack.

- serial shell can inspect toolchains, workspaces, jobs, and artifacts
- graphical terminal gets the same commands through `terminal-service`
- no special desktop-only developer path is introduced

That keeps developer workflows aligned with the same shell/runtime model instead
of forking a second developer UI surface too early.

## Package integration

`developer-service` is a normal package-delivered optional platform component.

Its package currently carries:

- the dynamic service manifest
- a developer catalog
- toolchain descriptors
- workspace descriptors
- sample source content
- SDK metadata placeholders

Installing the package activates the service through the normal
`package-service -> root-manager` path.

## Operator workflow

The current real workflow is:

1. `pkg install developer`
2. `dev toolchains`
3. `dev workspaces`
4. `dev workspace 0`
5. `dev build 0 native`
6. `dev build 0 linux`
7. `dev build 0 windows`
8. `dev build 0 macos`
9. `dev jobs`
10. `dev artifact <job-id>`

That proves:

- package-delivered developer infrastructure
- toolchain discovery
- workspace discovery
- build-job lifecycle logging
- cross-target artifact generation
- honest unsupported-target reporting

## Deferred

This phase intentionally does not implement:

- writable project directories and persistent build outputs
- full Rust/C/C++ language ecosystems
- local macOS SDK/sign/notarization support
- full Linux ABI/runtime execution of arbitrary foreign binaries
- Windows runtime support
- IDE/editor integration
- remote build farms or signing services
- stronger sandbox/container isolation for build workers
