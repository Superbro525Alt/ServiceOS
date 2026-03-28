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
  `audio-service`, `runtime-service`, `graphics-service`, `session-service`,
  and `desktop-shell-service`
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
- `desktop launch <settings|files|monitor>`
- `desktop launch <settings|files|monitor|terminal>`
- `desktop focus <settings|files|monitor|terminal>`
- `desktop next`
- `desktop close <settings|files|monitor|terminal>`
- `desktop minimize <settings|files|monitor|terminal>`
- `desktop restore <settings|files|monitor|terminal>`
- `desktop maximize <settings|files|monitor|terminal>`
- `desktop move <settings|files|monitor|terminal> <x> <y>`
- `desktop resize <settings|files|monitor|terminal> <width> <height>`
- `desktop click <x> <y>`
- `pkg list`
- `pkg info <name>`
- `pkg install <name> [version]`
- `pkg update <name> [version]`
- `pkg remove <name>`
- `pkg rollback <name>`
- `pkg history <name>`
- `runtime envs`
- `runtime create posix`
- `runtime inspect <env-id>`
- `runtime mounts <env-id>`
- `runtime vars <env-id>`
- `runtime runs`
- `runtime launch <env-id> <inspect|env|mounts|cat> [guest-path]`
- `runtime destroy <env-id>`
- `run sysinfo`

These commands intentionally exercise the real service contracts rather than
special shell-only backdoors.

Package commands call the real `package-service`, which then coordinates with
the root manager. The shell does not edit manifests or activate services by
itself.

Network commands call the real `network-service`.
The shell can inspect interfaces, resolve names, run probes, inspect active
transport sessions, and open a small outbound TCP stream session for HTTP
testing, but it never receives raw packet-interface authority directly.

Runtime commands call the real `runtime-service`.
The shell can create environments, inspect mapped resources, and launch
runtime-hosted workloads, but it does not gain ambient foreign-runtime power
or direct storage-root access.

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

## Deferred

- multiple sessions and routing policy
- job control and pipelines
- environment variables and richer process environments
- login/account/session ownership
- package-installed command discovery
- broader runtime-hosted command sets and desktop launch UX
- richer package UX and operator history views
- tabs, panes, and richer terminal emulation on top of the current
  terminal-service boundary
- richer graphical session tooling beyond the current desktop window controls
