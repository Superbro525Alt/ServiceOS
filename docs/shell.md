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
session-service
  -> graphics-service surfaces
       -> kernel display-output object
```

The shell opens a session by looking up `console-service` and requesting a
session channel. That session channel carries line-oriented input and raw text
output. The shell does not talk directly to the kernel debug console except
through `console-service`.

Current properties:

- one text-first operator session
- line-based input
- explicit session handle passing to transient tools
- no ambient login or account model yet

## Shell authority

The shell is intentionally not all-powerful.

It has:

- lookup rights for `console-service`, `log-service`, `config-service`,
  `storage-service`, `status-service`, `package-service`, `network-service`,
  `graphics-service`, and `session-service`
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
- `gfx outputs`
- `gfx surfaces`
- `gfx sessions`
- `gfx focus <surface-id>`
- `pkg list`
- `pkg info <name>`
- `pkg install <name> [version]`
- `pkg update <name> [version]`
- `pkg remove <name>`
- `pkg rollback <name>`
- `pkg history <name>`
- `run sysinfo`

These commands intentionally exercise the real service contracts rather than
special shell-only backdoors.

Package commands call the real `package-service`, which then coordinates with
the root manager. The shell does not edit manifests or activate services by
itself.

Graphics commands call the real `graphics-service` and `session-service`.
The shell can inspect and request focus changes because its manifest grants
lookup access; it does not own display hardware directly.

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

## Deferred

- multiple sessions and routing policy
- job control and pipelines
- environment variables and richer process environments
- login/account/session ownership
- package-installed command discovery
- richer package UX and operator history views
- richer terminal emulation and graphical shells
- richer graphical session tooling and window-management commands
