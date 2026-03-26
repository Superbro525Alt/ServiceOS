# Desktop Shell and Core Apps

## Role

The current desktop layer is the first product-facing shell on top of the
existing graphics/session platform.

It is intentionally not part of:

- the kernel
- `graphics-service`
- `session-service`
- the root manager

That separation matters. The compositor owns presentation mechanics, the
session service owns session/focus state, the root manager owns launch and
supervision, and the desktop shell owns product UX.

## Current structure

```text
root-manager
  -> graphics-service
  -> session-service
  -> desktop-shell-service
       owns desktop chrome, launcher, and app launch policy
       launches transient graphical apps through root-manager
  -> shell-service
       remains the text-first operator environment
```

Current graphical apps:

- `settings-app`
- `files-app`
- `monitor-app`

These are applications, not hidden platform services. They render into a
surface capability that the desktop shell creates for them and they receive
only their own explicit service handles.

## Desktop shell responsibilities

`desktop-shell-service` currently owns:

- desktop background and shell chrome surfaces
- launcher and status presentation
- app list and focused-app state
- app launch and focus requests over a dedicated desktop-shell contract
- manager-mediated app launch requests

It does not own:

- framebuffer or output hardware
- focus policy implementation
- direct task spawning
- ambient access to storage, config, package, or network policy

## Platform boundaries

The current split is:

- `graphics-service`
  - output discovery
  - surface creation
  - retained-scene composition
  - framebuffer presentation
- `session-service`
  - graphical session identity
  - focused-surface tracking
  - session status queries
- `desktop-shell-service`
  - launcher, chrome, app routing, and desktop status
- core apps
  - app-specific views over existing platform services

This keeps desktop product behavior replaceable in principle. A later shell can
reuse the same graphics/session contracts without rewriting the compositor or
the kernel.

## Launch and lifecycle model

Graphical apps are launched through the root manager, not directly by the
desktop shell.

Current flow:

1. `desktop-shell-service` creates a surface through `graphics-service`
2. it sends `ManagerTag::LaunchRequest` to the root manager
3. the root manager validates that `desktop-shell-service` may launch the
   requested app image
4. the root manager transfers the surface handle plus explicit service handles
   into the app startup channel
5. the app renders into its surface and remains a normal isolated task
6. the desktop shell watches the task handle and updates desktop state when the
   app exits

The desktop shell gets launch authority for a small known app set today, but
the authority path is explicit and manager-mediated rather than ambient.

## Capability model

The desktop layer keeps capability scoping central.

`desktop-shell-service` currently gets:

- a startup-granted log handle
- lookup access to `graphics-service`
- lookup access to `session-service`
- lookup access to `network-service`
- lookup access to `status-service`
- manager authorization to launch only desktop app images

Current per-app grants:

- `settings-app`
  - surface handle
  - log handle
  - `config-service`
  - `network-service`
- `files-app`
  - surface handle
  - log handle
  - `storage-service`
- `monitor-app`
  - surface handle
  - log handle
  - `status-service`
  - `network-service`

Apps do not inherit:

- the desktop shell control channel
- package authority
- storage root access
- graphics output ownership

## Core apps

The first app set is intentionally small and platform-validating.

- `settings-app`
  - reads live config and network values through service contracts
- `files-app`
  - lists persisted boot-store paths through `storage-service`
- `monitor-app`
  - shows heartbeat and network status through `status-service` and
    `network-service`

This is enough to prove that storage, status, network, and session-backed
rendering can all be exercised as real apps instead of shell-only diagnostics.

## Operator coexistence

The graphical desktop does not replace the serial/operator path.

- `shell-service` remains available on the console session
- shell commands can inspect desktop state through `desktop-shell-service`
- the desktop shell and serial shell are separate product layers over the same
  platform

That coexistence is intentional. It keeps bring-up and debugging usable while
the graphical product layer matures.

## Current limitations

The current desktop layer is real, but still early:

- shell chrome is intentionally simple
- focus changes are service-driven rather than pointer/keyboard driven
- there is no full window management policy yet
- apps use compositor scene primitives instead of shared buffers
- there is no notification center, dock, settings editor, or file-opening
  workflow yet

## Deferred

- physical input-device hosts and routed pointer/keyboard input
- richer task switching, window movement, and resize policy
- notifications and richer system-status UX
- package/software-center UI
- richer file workflows and open-with policy
- broader graphical app model and toolkit work
- desktop permissions UX and user-facing security prompts
