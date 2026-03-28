# Graphics and Session Platform

## Current model

The graphics/session layer is now a real userspace platform slice rather than a
bootstrap framebuffer hack.

The current stack is:

```text
kernel
  -> display-output object
root-manager
  -> graphics-service
       owns output state, surfaces, and composition
  -> session-service
       owns session identity and focus policy
  -> shell-service
       inspects graphics/session state through service contracts
```

The kernel still provides only mechanism:

- boot-time framebuffer discovery from the current platform boot parser
- a `DisplayOutput` kernel object with explicit rights
- syscalls to query output state and present a frame

Everything above that stays in userspace.

## Display-output model

The current display backend is a boot framebuffer captured during UEFI bring-up
and re-exposed as a kernel display-output object.

Public display state currently includes:

- backend type
- connection state
- pixel format
- width and height
- stride and bytes per pixel
- presentation count

The public service contract is intentionally backend-neutral. It describes
outputs and presentation state, not GOP-specific behavior, so later display
hosts can sit behind the same userspace boundary.

Backend placement now looks like this:

- `kernel/core/display`: display-output object contract
- `platform/x86_64/qemu_virtio/display`: current boot-framebuffer backend
- `platform/aarch64/raspi5/framebuffer`: Raspberry Pi 5 display scaffold only
- `userspace/programs/graphics-service`: surface ownership and composition

## Graphics service

`graphics-service` owns:

- display-output capability consumption
- output status reporting
- surface creation and tracking
- composition ordering
- framebuffer presentation

The current compositor is still deliberately simple:

- one output
- shell-owned retained-scene primitives
- client-owned mapped presentation buffers
- broader present requests on top of the surface contract
- userspace-owned z-order

This is enough to prove the client-facing surface and presentation model
without pretending that the final desktop compositor already exists.

## Surface model

The first surface contract now includes:

- surface creation through `graphics-service`
- per-surface capability handles
- geometry updates
- fill-color updates
- shared-buffer attachment through a transferred memory-object capability
- mapped buffer presentation requests from clients
- visibility updates
- surface status queries

Surface handles are explicit capabilities. Clients do not gain ambient access
to all graphical objects just because they can talk to the graphics service.

The current surface implementation supports two composition styles behind the
same contract:

- retained-scene primitives (`fill`, `rect`, `label`) owned by
  `graphics-service`
- client-owned mapped buffers backed by writable kernel memory objects

The client-render path is no longer just “attach a buffer and hope the
compositor copies it later.” Clients can now:

- map their own memory-object-backed pixel buffers
- render directly into those buffers
- issue explicit present-buffer requests through the graphics contract
- keep buffer ownership and lifetime explicit

That gives the system a real client-render substrate without collapsing the
compositor boundary into ad hoc shared-memory shortcuts.

Current mapped-buffer clients include:

- `monitor-app`
- `files-app`
- `terminal-app`

The desktop shell still owns chrome and policy, while apps own their local
pixel content. This is the right long-term split for richer client rendering.

## Session model

`session-service` owns session identity and focus policy on top of
`graphics-service`.

The current session model is intentionally small:

- one graphical session
- service-controlled focus routing
- focus changes expressed as surface selection
- session status queries through a dedicated service contract

This establishes the authority boundary now:

- `graphics-service` owns surface and presentation mechanics
- `session-service` owns session and focus policy

That split is important because later login, desktop shell, and multi-session
policy should extend `session-service` rather than leaking into the compositor.

## Capability model

Graphics/session access is explicit.

- the kernel gives the root manager one display-output capability
- the root manager passes that capability only to `graphics-service`
- `session-service` talks to `graphics-service` through a lookup-authorized
  service handle
- the shell gets send-only lookup access to inspect graphics/session state
- per-surface control flows through surface-specific handles returned by
  `graphics-service`

There is no ambient "draw anywhere" or "own the display" authority.

## Operator path

The shell now exposes the real service contracts with:

- `gfx outputs`
- `gfx surfaces`
- `gfx sessions`
- `gfx focus <surface-id>`

These commands do not bypass the service model. They talk to
`graphics-service` and `session-service` through the same lookup-mediated path
used by the rest of the platform.

## Bring-up backend and portability

Generic and durable parts of the current design:

- kernel display-output object
- graphics/session service split
- explicit surface handles
- explicit session/focus service
- operator inspection through service contracts

Bring-up-specific parts:

- `platform/x86_64/qemu_virtio::boot` UEFI framebuffer discovery
- one boot framebuffer backend
- one software-composited output
- one VirtIO input bring-up path for pointer and keyboard delivery

This is intentionally QEMU-friendly but not QEMU-locked. Later work can add
real display, GPU, and input backends without redefining the public service
contracts.

## Deferred

Still intentionally deferred:

- multiple outputs
- multiple graphical sessions
- damage tracking and multi-buffer client presentation
- richer client-render protocol details beyond the current mapped-buffer present
  path
- GPU acceleration and richer display mode management
