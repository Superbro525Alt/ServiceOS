# Audio and Media Platform Services

## Scope

The current audio layer is the first real media-platform slice above the
kernel's low-level device primitives. It is intentionally modest, but it is no
longer "audio missing":

- one userspace `audio-service`
- one kernel audio-endpoint object type
- explicit output endpoint discovery and status
- session-tagged playback stream creation and lifecycle
- simple routing over the current default output endpoint
- operator and app-facing playback through the real service contract

This is not yet a full PCM mixer, capture stack, or media framework. It is the
first durable platform boundary for later playback, notifications, voice, and
per-app media policy.

## Service model

`audio-service` owns audio policy in userspace.

It is responsible for:

- consuming the explicit bootstrap audio-endpoint capability from the root
  bootstrap path
- exposing endpoint status and playback-stream operations over its public
  channel
- associating streams with session ids
- enforcing stream lifecycle and endpoint arbitration
- logging endpoint and stream state transitions

It is not responsible for:

- kernel-resident audio policy
- desktop-shell-owned playback shortcuts
- per-app UI policy
- advanced codec, mixing, or media-library behavior

## Kernel and backend boundary

The kernel exposes a backend-neutral audio-endpoint object. That object
supports:

- endpoint info queries
- tone playback requests
- explicit stop requests

The current x86_64 bring-up path implements that object with a QEMU-compatible
PC speaker backend in `platform/x86_64/qemu_virtio/audio`.

Backend placement now looks like this:

- `kernel/core/audio`: generic audio-endpoint contracts and object semantics
- `platform/x86_64/qemu_virtio/audio`: PC speaker endpoint backend
- `userspace/programs/audio-service`: endpoint, stream, and routing policy

The public contract is intentionally backend-agnostic. The current backend is a
tone-capable output endpoint, not a promise that all future audio will look
like the PC speaker.

## Endpoint model

The endpoint model is deliberately small and explicit:

- endpoints have a stable numeric index
- endpoints report backend, direction, state, capability flags, nominal rate,
  channel count, supported tone range, current tone frequency, and play count
- the first platform slice currently exposes one output endpoint

The current endpoint states are:

- `offline`
- `idle`
- `active`

The current backend reports:

- backend: `pc-speaker`
- direction: `output`
- capabilities: playback + tone

That is enough to validate the service boundary now without hardcoding future
hardware assumptions into clients.

## Stream and routing model

`audio-service` exposes playback streams as dedicated per-stream channel
handles.

The current stream model provides:

- `stream open`
- `stream list`
- `stream status`
- `stream play tone`
- `stream close`

Each stream carries:

- stream slot id
- stream direction
- stream state
- owning session id
- endpoint index
- active frequency
- remaining ticks when queried directly

The current routing policy is intentionally simple:

- one default output endpoint
- one active tone at a time on the current backend
- a new active playback request preempts the older active stream

That keeps the public model durable while avoiding a fake "advanced mixer"
claim before PCM and richer device backends exist.

## Session-aware model

Streams are associated with session ids at creation time.

The current platform does not yet expose per-app volume controls or focus-based
ducking, but the stream metadata already preserves the boundary needed for that
later work:

- session ownership is recorded in the service
- operator tools can inspect active streams and their session ids
- apps do not talk directly to kernel audio objects

This gives the platform a clean path toward later desktop media controls
without pushing that policy into the shell or kernel.

## Capability model

Audio access is explicit.

- the kernel gives the root manager one bootstrap audio-endpoint capability
- the root manager passes that raw endpoint only to `audio-service`
- other services and apps do not receive raw hardware authority
- clients use `audio-service` through normal service lookup or launch-granted
  handles
- `settings-app` receives a send-only `audio-service` handle as an explicit
  launch grant

That means playback authority is service-mediated rather than ambient.

## Desktop and operator integration

The first platform integration points are intentionally small but real:

- `settings-app` shows audio endpoint status and exposes a `TEST TONE` control
  through the real `audio-service` contract
- `shell-service` exposes:
  - `audio endpoints`
  - `audio streams`
  - `audio tone <hz> [ms]`

Those commands and UI flows all use the same userspace service boundary.

## End-to-end workflow

The current `qemu-virtio` operator workflow is:

1. the kernel creates the platform audio endpoint during bootstrap
2. the root manager passes that raw endpoint only to `audio-service`
3. `audio-service` registers and logs `audio-endpoint-ready`
4. `audio endpoints` reports the live endpoint state
5. `audio tone 880 120` opens a stream, starts playback, stops it, and closes
   the stream through the real service path

The default QEMU run path configures the PC speaker backend against a WAV sink,
so the current bring-up path is both observable in logs and captureable from
the host.

## Roadmap note

Open audio and media follow-on work is tracked centrally in
[docs/roadmap.md](roadmap.md). This page intentionally stays focused on the
current audio-service architecture and implemented behavior.
