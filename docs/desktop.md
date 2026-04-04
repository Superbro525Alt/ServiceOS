# Desktop Interaction and Core Apps

## Role

The current desktop layer is the first product-facing shell on top of the
existing graphics and session platform.

It is intentionally not part of:

- the kernel
- `graphics-service`
- `session-service`
- the root manager

That separation matters. The compositor owns presentation mechanics, the
session service owns session identity and focused-surface state, the root
manager owns launch authorization and supervision, and the desktop shell owns
desktop interaction policy.

## Current structure

```text
root-manager
  -> graphics-service
  -> session-service
  -> desktop-shell-service
       owns desktop chrome, launcher, window management, and app launch policy
       launches transient graphical apps through root-manager
  -> shell-service
       remains the text-first operator environment
```

Current graphical apps:

- `settings-app`
- `files-app`
- `monitor-app`
- `terminal-app`

These are applications, not hidden platform services. They render into a
surface capability created by the desktop shell, receive one app-control
channel for desktop-driven events, and get only their own explicit service
handles.

## Desktop shell responsibilities

`desktop-shell-service` currently owns:

- desktop background and shell chrome surfaces
- launcher and status presentation
- command-palette, task-switcher, notification-center, and clipboard-history overlays
- retained per-window state
- app list, focused-app state, and z-order
- MRU app-switch history and launcher ranking hints
- workspace assignment and active-workspace state
- move, edge or corner resize, minimize, maximize, restore, and close policy
- pointer hit testing for launcher, titlebar, controls, and content regions
- keyboard focus and app-target routing on top of session focus
- app launch and window-action requests over the desktop-shell contract
- file handoff into the files app through the desktop-shell contract
- manager-mediated app launch requests
- notifications presentation, timeout state, and retained history
- richer shortcut handling, command routing, and task switching on top of the
  focused-window model

It does not own:

- framebuffer or output hardware
- the mechanics of surface composition
- raw session identity
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
  - focused-surface tracking for the active session
  - session status queries
- `desktop-shell-service`
  - launcher, chrome, window management, app routing, and desktop status
- core apps
  - app-specific views over existing platform services
  - app-local rendering and response to app-control events

This keeps desktop product behavior replaceable in principle. A later shell can
reuse the same graphics and session contracts without rewriting the compositor
or the kernel.

## Launch and lifecycle model

Graphical apps are launched through the root manager, not directly by the
desktop shell.

Current flow:

1. `desktop-shell-service` creates a surface through `graphics-service`
2. it duplicates a rights-reduced app surface handle and creates an
   app-control channel pair
3. it sends a launch request to the root manager
4. the root manager validates that `desktop-shell-service` may launch the
   requested app image
5. the root manager transfers the app-visible surface handle, the app-control
   handle, and explicit service handles into the app startup channel
6. the app renders into its surface and remains a normal isolated task
7. the desktop shell retains the authoritative window state and watches the
   task handle for cleanup

The desktop shell gets launch authority for a small known app set today, but
the authority path is explicit and manager-mediated rather than ambient.

## Window-management model

The first real window-management layer now lives in `desktop-shell-service`.

Per-window state includes:

- owning desktop app id
- retained surface handle
- position and size
- focused or unfocused state
- minimized or visible state
- maximized or restored state
- z-order

Current interaction rules:

- launching or restoring an app focuses it and raises it to the top
- only one desktop app is focused at a time
- titlebar drag moves the window
- any window edge or corner drag resizes the window
- close, minimize, and maximize buttons are explicit titlebar hit targets
- minimizing removes the window from normal hit testing and focus rotation
- maximizing expands the window to the session work area and restore returns it
  to its prior geometry
- closing notifies the app through the app-control channel and then cleans up
  the retained surface/task state

This keeps window policy out of `graphics-service`. The compositor only knows
about surfaces; it does not decide which surface should behave like a desktop
window.

## Input and focus model

The current input path is now physical and session-routed.

- the kernel exposes one explicit input-source capability
- `session-service` receives that capability from the root bootstrap path
- `session-service` translates hardware pointer and keyboard events into the
  desktop interaction contract
- `desktop-shell-service` hit-tests shell chrome versus window content,
  updates focus, and applies window-management policy
- focused-window keyboard events and app-content pointer events go out over the
  app-control channel; apps still do not receive global input authority

Current behavior:

- pointer click focuses and raises the target window
- titlebar drag moves a window
- edge and corner drags resize a window
- titlebar controls close, minimize, and maximize or restore a window
- app-content pointer events are routed only to the owning app window
- `Alt+Tab` cycles focused windows in MRU order and shows a switcher overlay
- `Alt+Shift+Tab` cycles backward
- `Alt+1..4` launches or focuses the primary desktop apps
- `Alt+F4` requests close on the focused app
- `Ctrl+Space` opens the command palette
- `Alt+N` opens notification history
- `Ctrl+Alt+V` opens clipboard history
- `Ctrl+Alt+1..4` switches workspaces
- `Ctrl+Alt+Shift+1..4` moves the focused app to another workspace
- text input reaches the focused app when that app chooses to consume it

The desktop-shell contract now also exposes:

- workspace inspection and switching
- notification history inspection
- file handoff into the files app through `desktop open <path>`

This establishes durable interaction boundaries:

- session identity stays in `session-service`
- composition stays in `graphics-service`
- desktop interaction policy stays in `desktop-shell-service`
- app-local behavior stays in the apps

Future physical input hosts can feed this same desktop interaction contract
without redesigning the product-layer boundary.

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
  - app-control channel
  - `config-service`
  - `network-service`
- `files-app`
  - surface handle
  - app-control channel
  - `storage-service`
- `monitor-app`
  - surface handle
  - app-control channel
  - `status-service`
  - `network-service`
- `terminal-app`
  - surface handle
  - app-control channel
  - `terminal-service`

The shell itself also consumes:

- `clipboard-service`
  - retained clipboard history display and activation
- `security-service`
  - existing launch policy and denial surfacing, still enforced outside the
    desktop shell

Apps do not inherit:

- the desktop shell control channel
- package authority
- storage root access
- graphics output ownership

## Current deferred work

Open follow-on work lives in [docs/roadmap.md](/home/paulh/os-dev/docs/roadmap.md).
The current desktop shell now has workspaces, MRU task switching, command
palette search, notification history, clipboard history, and basic file
handoff, but it still defers:

- multi-monitor shell policy
- drag-and-drop
- broader open-with and default-association policy
- smoother animations, snap or tiling, and gesture policy
- richer permissions-aware desktop flows
- high-contrast, zoom, and broader assistive hooks
- global input-routing power
- other apps' surface handles

## Core apps

The first app set is intentionally small and platform-validating.

- `settings-app`
  - reads live config and network values through service contracts
- `files-app`
  - lists persisted boot-store paths through `storage-service`
- `monitor-app`
  - shows heartbeat and network status through `status-service` and
    `network-service`
- `terminal-app`
  - hosts the shared shell/runtime stack through `terminal-service` inside a
    desktop window

Each app now also proves a piece of desktop lifecycle behavior:

- repaint on focus changes
- repaint on desktop-driven resize events
- clean exit on desktop close

`settings-app` now also provides the first real app-local text entry path. The
desktop shell forwards pointer, key, and text events over the app-control
channel, and the app updates its own note field without gaining session-global
input authority.

The desktop shell also now exposes a first notification path through the real
desktop contract. Shell/operator commands can post notifications without
gaining compositor or app-owned surface authority, and app faults can surface
as desktop notifications instead of disappearing silently.

## Operator coexistence

The graphical desktop does not replace the serial/operator path.

- `shell-service` remains available on the console session
- shell commands can inspect desktop status, windows, and app state through
  `desktop-shell-service`
- the serial shell can request launch, focus, move, resize, minimize, restore,
  maximize, close, and pointer-click actions through the same desktop contract
- the desktop shell and serial shell remain separate product/operator layers
  over the same platform

That coexistence is intentional. It keeps bring-up and debugging usable while
the graphical product layer matures.

## Current limitations

The current desktop layer is real, but still early:

- shell chrome is intentionally simple
- most apps still use compositor scene primitives; `monitor-app` now exercises
  the shared-buffer client-draw path
- there is no notification center, dock, settings editor, or file-opening
  workflow yet
- there is no snap, tiling, or animation layer yet
- there is only one physical input source and one graphical session today
- global shortcuts are intentionally minimal and app text input is still
  primitive

## Roadmap note

Open desktop-shell and graphical-UX follow-on work is tracked centrally in
[docs/roadmap.md](roadmap.md). This page intentionally stays focused on the
current desktop architecture and implemented behavior.
