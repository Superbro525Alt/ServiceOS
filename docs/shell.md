# Shell and Session Model

## Role

`shell-service` is the first operator-facing environment in the system.

It exists to make the service platform usable from within itself without
turning the shell into a privileged grab-bag. The shell is just another
service with explicit capabilities and one manager-authorized control channel.

## Session model

The current session stack is:

```text
shell-service
  -> console-service session
       -> raw serial console path
terminal-service
  -> terminal session
       -> terminal-app window
session-service
  -> graphics-service surfaces
       -> kernel display-output object
```

The shell opens a session by looking up `console-service` and requesting a
session channel. That session channel carries line-oriented input and raw text
output. The shell does not talk directly to the kernel debug console except
through `console-service`.

The same shell command/runtime layer is also reused by `terminal-service`,
which exposes a PTY-like terminal session contract to the graphical
`terminal-app`. That keeps shell semantics shared while leaving console and
graphical hosting as separate presentation layers.

Current properties:

- one text-first operator session
- one or more graphical terminal sessions through `terminal-service`
- line-based input
- explicit session handle passing to transient tools
- no ambient login or account model yet

## Shell authority

The shell is intentionally not all-powerful.

It has:

- lookup rights for `console-service`, `log-service`, `config-service`,
  `storage-service`, `status-service`, `package-service`, `network-service`,
  `audio-service`, `runtime-service`, `developer-service`, `security-service`,
  `graphics-service`, `session-service`, and `desktop-shell-service`
- its own bootstrap/control channel back to the root manager
- manager authorization for service restart and transient tool launch

It does not have:

- ambient storage-root authority
- direct service spawn privilege
- direct kernel-management privilege
- unrestricted access to services outside its manifest policy

## Commands

Current built-in commands:

- `help`
- `services`
- `service <name>`
- `restart <name>`
- `logs [count]`
- `config`
- `store ls [prefix]`
- `store mounts`
- `store mkdir <path>`
- `store write <path> <text>`
- `store rm <path>`
- `cat <path>`
- `status`
- `net ifaces`
- `net route`
- `net sockets`
- `net resolve <name>`
- `net ping <name|ip>`
- `net http <host> [path]`
- `gfx outputs`
- `gfx surfaces`
- `gfx sessions`
- `gfx focus <surface-id>`
- `desktop status`
- `desktop apps`
- `desktop windows`
- `desktop workspace [status]`
- `desktop workspace switch <1-4>`
- `desktop workspace move <1-4>`
- `desktop notifications [count]`
- `desktop launch <settings|files|monitor|terminal|software>`
- `desktop focus <settings|files|monitor|terminal|software>`
- `desktop next`
- `desktop close <settings|files|monitor|terminal|software>`
- `desktop minimize <settings|files|monitor|terminal|software>`
- `desktop restore <settings|files|monitor|terminal|software>`
- `desktop maximize <settings|files|monitor|terminal|software>`
- `desktop move <settings|files|monitor|terminal|software> <x> <y>`
- `desktop resize <settings|files|monitor|terminal|software> <width> <height>`
- `desktop click <x> <y>`
- `desktop notify <text>`
- `desktop open <path>`
- `run image <path>`
- `pkg list`
- `pkg catalog`
- `pkg repos`
- `pkg repo add <name> <url> [unsigned|pinned:<hex>] [stable|beta|canary] [production|preview|testing]`
- `pkg repo sync [all|index]`
- `pkg info <name>`
- `pkg install <name> [version]`
- `pkg update <name> [version]`
- `pkg remove <name>`
- `pkg rollback <name>`
- `pkg history <name>`
- `pkg provenance <name>`
- `pkg policy <name>`
- `pkg pin <name> <version|none>`
- `pkg channel <name> <stable|beta|canary>`
- `pkg ring <name> <production|preview|testing>`
- `pkg verify`
- `pkg repair`
- `pkg gc`
- `runtime envs`
- `runtime create posix`
- `runtime inspect <env-id>`
- `runtime mounts <env-id>`
- `runtime vars <env-id>`
- `runtime runs`
- `runtime launch <env-id> <inspect|env|mounts|cat> [guest-path]`
- `runtime destroy <env-id>`
- `security apps`
- `security app <name> [allow|block|default]`
- `security runtimes`
- `security runtime <env-id> [approve|deny|reset]`
- `security repos`
- `security package <name>`
- `security workspace <id>`
- `security audit [count]`
- `dev toolchains`
- `dev toolchain <id>`
- `dev workspaces`
- `dev workspace <id>`
- `dev build <workspace-id> <native|linux|windows|macos>`
- `dev jobs`
- `dev artifact <job-id>`
- `run sysinfo`

These commands intentionally exercise the real service contracts rather than
special shell-only backdoors.

Storage commands now open the composed namespace root through
`storage-service`, then traverse or mutate through scoped directory
capabilities. `store mounts` also exposes the current mount table from the same
service contract.

Package commands call the real `package-service`, which then coordinates with
the root manager. The shell does not edit manifests or activate services by
itself. Repository sync, provenance, policy, and repair/GC flows all use that
same contract.

Network commands call the real `network-service`.
The shell can inspect interfaces, resolve names, run probes, inspect active
transport sessions, and open a small outbound TCP stream session for HTTP
testing, but it never receives raw packet-interface authority directly.

Runtime commands call the real `runtime-service`.
The shell can create environments, inspect mapped resources, and launch
runtime-hosted workloads, but it does not gain ambient foreign-runtime power
or direct storage-root access.

Security commands call the real `security-service`, `runtime-service`, and
`package-service`.
The shell can review native app launch policy, inspect runtime approval state,
inspect repository/package trust state, and review security audit history, but
it does not bypass manager launch checks or package trust enforcement.

Developer commands call the real `developer-service`.
The shell can inspect packaged toolchains and workspaces, submit build jobs,
inspect job state, and open exported artifact handles, but it does not gain
ambient build-worker spawn power, storage-root access, or package-policy
ownership.

Graphics commands call the real `graphics-service` and `session-service`.
The shell can inspect and request focus changes because its manifest grants
lookup access; it does not own display hardware directly.

Desktop commands call the real `desktop-shell-service`.
The shell can inspect and request app launch, focus, window actions, and
desktop-pointer actions through that service, but it does not gain direct
graphical app-spawn authority, shell-owned surface handles, or app-local input
authority.

## Tool launch model

Transient tools are launched through the root manager.

Current flow:

1. the shell sends `ManagerTag::LaunchRequest` over its bootstrap/control
   channel
2. the shell may transfer its current session handle with reduced rights
3. the root manager spawns the requested image
4. the root manager passes the session handle into the tool startup channel
5. the root manager returns only a task handle to the shell
6. the shell waits for task exit through the normal task-status syscall path

This keeps launch policy in userspace and preserves a clean distinction between
long-running services and transient operator tools.

## Desktop coexistence

The serial shell remains a first-class operator path even after the graphical
desktop comes up.

- the graphical desktop owns product UX
- the serial shell owns low-level inspection and operator workflows
- the graphical terminal hosts the same shell/runtime stack inside the desktop
- both sit on top of the same root-manager and service contracts

That split is deliberate. The desktop can evolve without sacrificing bring-up
and debugging access.

## Roadmap note

Open shell and operator-workflow follow-on work is tracked centrally in
[docs/roadmap.md](roadmap.md). This page intentionally stays focused on the
current shared shell model and implemented commands.
