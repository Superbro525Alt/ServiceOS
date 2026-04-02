# Userspace Bootstrap

## Current scope

The userspace path now covers four layers:

- the kernel can construct and enter isolated user address spaces
- the kernel can launch a real root userspace service manager
- the root manager can bootstrap storage from a kernel-delivered boot-store
  capability
- the root manager can start a small always-on platform graph from persisted
  manifests
- the platform can host a real operator shell and launch transient tools
- the platform can expose a real userspace networking service backed by an
  explicit kernel packet-interface object
- the platform can expose a real userspace graphics/session layer backed by an
  explicit kernel display-output object
- the platform can host a real desktop shell and launch capability-scoped
  graphical apps on top of the same service graph

This is still intentionally early, but it is now a real service-composed
platform substrate rather than a single bootstrap demo.

## Executable format and runtime loading

Userspace binaries are built as freestanding `x86_64-unknown-none` programs and
can now be loaded as either:

- the original ServiceOS flat image format
- native ELF64 executable images

The current runtime path is:

1. `xtask` builds userspace programs and bundle data into a boot-store image.
2. `xtask` stages `bootstore.bin` on the EFI system partition.
3. the UEFI kernel image reads that boot-store file before
   `ExitBootServices`.
4. the kernel resolves built-in executable images from the boot store by
   `image_id`.
5. the kernel passes a read-only boot-store capability to the root manager.
6. the root manager starts `storage-service`.
7. `storage-service` exposes persisted manifests and resources back to the root
   manager as explicit blob capabilities.
8. the root manager can also read user-supplied stored images through
   `storage-service` and launch them through the same manager-owned spawn path.

The flat-image header carries:

- a magic value
- an ABI version
- an image base
- an entry offset
- a file-backed image byte count
- an executable span limit
- a writable-data start offset
- a total mapped image size
- a user stack top

The current loader is still intentionally simple even though it is broader than
before:

- ServiceOS flat images remain the default native service/tool format
- ELF64 `ET_EXEC` is now supported for native loading
- dynamic linking, relocations, and dependency loading remain deferred

## Address-space model

The current user address space is built by:

- allocating a new top-level page table
- copying the kernel-visible mappings needed for kernel entry and return
- mapping one flat-image region or one ELF load-segment set
- mapping one bootstrap user stack region

The loader now supports two native image shapes:

- one contiguous ServiceOS flat-image region, with an executable span and a
  writable data span carried in the flat-image header
- one or more ELF64 `PT_LOAD` segments mapped with executable/writable flags
  derived from the segment headers

It is still intentionally simpler than a full dynamic loader.

## Syscall ABI

Userspace enters the kernel through interrupt vector `0x80`.

Current syscall numbers:

- `0`: return the kernel syscall ABI version
- `1`: return the current monotonic tick count
- `2`: terminate the current thread with a supplied status code
- `3`: cooperatively yield the current thread
- `4`: write a debug log line through the kernel serial path
- `5`: create a channel pair
- `6`: send a channel message
- `7`: receive a channel message
- `8`: duplicate a handle with rights reduction
- `9`: close a handle
- `10`: spawn a boot-store service image with an explicit bootstrap authority
  capability
- `11`: query task exit status
- `12`: read from a kernel memory object
- `13`: read one byte from the raw debug console
- `14`: write bytes directly to the raw debug console
- `15`: query packet-interface status
- `16`: receive one packet frame from a packet-interface object
- `17`: transmit one packet frame through a packet-interface object
- `18`: query display-output status
- `19`: present one frame to a display-output object

This is enough for a real service manager and storage bootstrap without
pretending the kernel already exposes a full general-purpose process API.

## Current service graph

The root manager now brings up:

- `storage-service`
- `console-service`
- `config-service`
- `log-service`
- `network-service`
- `graphics-service`
- `session-service`
- `desktop-shell-service`
- `status-service`
- `shell-service`

That graph proves:

- persisted executable and manifest loading inputs
- dependency ordering
- startup capability grants
- startup-granted resource blobs
- controlled service discovery
- long-running service supervision
- structured logging through userspace services
- explicit networking authority routed through `network-service`
- explicit display authority routed through `graphics-service`
- session/focus policy split out into `session-service`
- a product-layer desktop shell above the graphics/session platform boundary
- manager-mediated graphical app launch with explicit per-app startup grants
- a text-first operator session layered on the service graph
- manager-mediated transient program launch

## Session and tool launch model

The shell does not get ambient kernel or filesystem power.

- `shell-service` opens a session through `console-service`
- shell commands inspect services through the root-manager bootstrap/control
  channel
- shell reads logs, config, and storage through the same capability-scoped
  service contracts as any other service
- transient tools are launched by the root manager on shell request
- tools can inherit only the session handle or other explicit capabilities that
  the shell passes through the manager

Graphical apps follow the same model:

- `desktop-shell-service` creates the app surface through `graphics-service`
- the desktop shell asks the root manager to launch a specific app image
- the root manager injects only the app's explicit startup handles
- the app renders into its assigned surface and remains a normal isolated task

## Roadmap note

Open userspace follow-on work is tracked centrally in
[docs/roadmap.md](roadmap.md). This page intentionally stays focused on the
current userspace structure and implemented boundaries.
