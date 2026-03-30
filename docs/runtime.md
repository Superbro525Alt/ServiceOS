# Compatibility and Runtime Foundations

## Role

`runtime-service` is the first compatibility/runtime platform service.

It exists to host non-native execution environments as explicit userspace
objects instead of letting foreign-runtime behavior leak into the native app
model.

It owns:

- runtime environment creation and destruction
- environment metadata and lifecycle state
- resource-view mapping for mounted storage and injected environment variables
- manager-mediated runtime workload launch
- runtime run inspection and teardown
- structured logging for environment and run lifecycle

It does not own:

- kernel process policy
- package installation policy
- desktop window policy
- raw storage authority
- ambient access to network, graphics, input, or audio

## Service model

The current path is intentionally small but real:

```text
shell / terminal / future desktop client
  -> runtime-service
       -> root-manager transient launch path
            -> runtime-hosted tool image
```

`runtime-service` is package-delivered rather than part of the always-on base
graph.

That keeps compatibility support explicit:

- it is installed and activated through `package-service`
- it runs as a normal userspace service with an ordinary manifest
- clients discover it through the manager like any other service

The first runtime kind is `posix`.

That does not mean Linux ABI compatibility is finished. It means the platform
now has a durable compatibility boundary that can host Linux-oriented runtime
growth later.

## Environment model

Each runtime environment currently tracks:

- `runtime kind`
- `env state`
- `capability flags`
- `mount table`
- injected `KEY=VALUE` variables
- active run count

Current environment states are:

- `ready`
- `busy`
- `destroyed`

Current runtime kinds are:

- `posix`

The public model is already shaped so later runtime kinds can be added without
redesigning the service contract.

## Resource mapping

Runtime environments do not receive ambient filesystem access.

Instead, `runtime-service` owns a small resource-view model:

- guest paths such as `/runtime`
- mapped storage sources such as `packages/runtime-service/...`
- injected environment variables such as `PATH`, `HOME`, and `TERM`
- explicit capability flags such as `file-read` and `terminal-io`

The first packaged runtime profile is:

- kind: `posix`
- caps: `file-read,terminal-io`
- mount: `/runtime -> packages/runtime-service/1.0.0/runtime/root`

That keeps the runtime view explicit and inspectable while avoiding fake
global-host path semantics.

## Launch model

Runtime workloads are launched through the existing manager-mediated transient
tool path.

Current flow:

1. a client asks `runtime-service` to launch a workload in an environment
2. `runtime-service` validates the environment and requested workload kind
3. `runtime-service` creates a runtime session/relay channel for output
4. `runtime-service` asks the root manager to launch the hosted tool image
5. the root manager spawns the tool and delivers only the startup handles and
   payload that were explicitly requested
6. the tool exits normally and `runtime-service` updates run state

The first hosted tool is `posix-host-tool`.

It is deliberately small. Its job is to prove the environment model and launch
path, not to pretend the platform already runs arbitrary Linux binaries.

Current supported workload kinds are:

- `inspect`
- `env`
- `mounts`
- `cat <guest-path>`

## Capability model

Compatibility workloads remain capability-scoped.

Current rules:

- clients only gain runtime authority by looking up `runtime-service`
- `runtime-service` only gets the storage and log access declared in its
  manifest
- hosted workloads only receive the handles that `runtime-service` explicitly
  transfers for that launch
- the first profile only grants `file-read` and `terminal-io`

That means the current runtime foundation does not implicitly grant:

- raw storage root access
- network access
- graphics/window authority
- input authority
- audio authority

Later runtime profiles can extend that model by adding explicit capability
flags and startup-handle grants instead of changing the native platform model.

## Terminal and desktop integration

The shared shell can drive runtime environments from either:

- the serial console path
- the graphical terminal path

That reuse is intentional. Runtime commands belong to the shared shell/runtime
layer, not to a desktop-only compatibility UI.

Current shell commands are:

- `runtime envs`
- `runtime create posix`
- `runtime inspect <env-id>`
- `runtime mounts <env-id>`
- `runtime vars <env-id>`
- `runtime runs`
- `runtime launch <env-id> <inspect|env|mounts|cat> [guest-path]`
- `runtime destroy <env-id>`

The desktop shell does not gain special compatibility power. Future
runtime-hosted desktop apps can be added later through explicit launch and
session contracts.

## Package integration

The first runtime foundation is packaged.

That proves:

- compatibility support can be distributed as a first-class package
- runtime metadata and root content can ship as package resources
- activation goes through the same package and root-manager contracts as other
  optional services

The current `runtime-service` package includes:

- a package manifest
- a service manifest
- a runtime profile resource
- a small mounted runtime root tree

## Observability

`runtime-service` emits structured lifecycle events for:

- environment creation
- environment destruction
- launch start
- launch exit
- mapped file reads

The shell can inspect:

- current environments
- mount tables
- injected environment variables
- active and exited runs

That keeps compatibility/runtime behavior observable without turning the shell
into a privileged shortcut path.

## Verified workflow

The current end-to-end workflow is:

1. `pkg install runtime`
2. `runtime create posix`
3. `runtime inspect 0`
4. `runtime mounts 0`
5. `runtime vars 0`
6. `runtime launch 0 inspect`
7. `runtime launch 0 env`
8. `runtime launch 0 mounts`
9. `runtime launch 0 cat /runtime/etc/runtime-release`
10. `runtime runs`
11. `runtime destroy 0`

That proves package-backed activation, environment creation, explicit resource
mapping, manager-mediated launch, output relay, run inspection, and teardown.

## Roadmap note

Open compatibility/runtime follow-on work is tracked centrally in
[docs/roadmap.md](roadmap.md). This page intentionally stays focused on the
current runtime-service foundation and implemented behavior.
