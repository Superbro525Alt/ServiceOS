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

- boot-time framebuffer discovery from UEFI GOP
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

## Graphics service

`graphics-service` owns:

- display-output capability consumption
- output status reporting
- surface creation and tracking
- composition ordering
- framebuffer presentation

The current compositor is deliberately simple:

- one output
- solid-fill rectangular surfaces
- full-frame recomposition on change
- userspace-owned z-order

This is enough to prove the client-facing surface and presentation model
without pretending that the final desktop compositor already exists.

## Surface model

The first surface contract includes:

- surface creation through `graphics-service`
- per-surface capability handles
- geometry updates
- fill-color updates
- visibility updates
- surface status queries

Surface handles are explicit capabilities. Clients do not gain ambient access
to all graphical objects just because they can talk to the graphics service.

The current surface implementation does not yet expose shared buffers, image
uploads, or application-defined drawing protocols. Those are later extensions
to the same object model, not reasons to change it.

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

- UEFI GOP framebuffer discovery
- one boot framebuffer backend
- full-frame copies on presentation
- one VirtIO input bring-up path for pointer and keyboard delivery

This is intentionally QEMU-friendly but not QEMU-locked. Later work can add
real display, GPU, and input backends without redefining the public service
contracts.

## Deferred

Still intentionally deferred:

- shared-memory or zero-copy presentation buffers
- multiple outputs
- multiple graphical sessions
- window management policy
- desktop shell, launcher, dock, notifications, and settings UI
- GPU acceleration and richer display mode management
