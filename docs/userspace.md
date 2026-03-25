# Userspace Bootstrap

## Current scope

The userspace path now covers two layers:

- the kernel can construct and enter isolated user address spaces
- the kernel can launch a real root userspace service manager that builds the
  first service graph

This is still intentionally early, but it is no longer just a single demo
program.

## Executable format and image catalog

Userspace binaries are built as freestanding `x86_64-unknown-none` programs and
packed into a deliberately small flat image format owned by `kernel/core::user`.

The current kernel image does not discover executables from storage. Instead it
links against a host-built userspace catalog that resolves image IDs to built-in
flat images.

The flat-image header carries:

- a magic value
- an ABI version
- an image base
- an entry offset
- an image byte count
- a user stack top

Why this is still flat instead of ELF:

- it keeps this stage focused on privilege transition, address-space ownership,
  and service bootstrap
- it avoids dragging relocation, segment, and filesystem policy into the kernel
- it gives later phases a replaceable loader boundary instead of baking storage
  policy into the kernel too early

## Address-space model

The current user address space is built by:

- allocating a new top-level page table
- copying the kernel-visible mappings needed for kernel entry and return
- mapping one flat user image region
- mapping one bootstrap user stack region

The loader still maps the flat image as one contiguous user region. Fine-grained
segment permissions and user-visible VM policy remain deferred.

## Syscall ABI

Userspace currently enters the kernel through interrupt vector `0x80`.

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
- `10`: spawn a built-in service image from the bootstrap root
- `11`: query task exit status

This is enough for a real root service manager without pretending the kernel
already exposes a broad general-purpose process API.

## Root manager bootstrap

The first userspace task is now the root service manager. The current bootstrap
contract is:

- the kernel launches the built-in root-manager image
- the root manager runs as the bootstrap-root task
- it can spawn child services through the current bootstrap syscall
- child services receive bootstrap control channels and explicit startup grants
- registration and discovery remain manager-owned userspace policy

The first service graph consists of:

- `log-service`
- `echo-service`
- `probe-service`

That graph proves dependency ordering, explicit capability distribution,
registry-mediated discovery, and restart supervision.

## Still deferred

The current userspace layer still does not include:

- ELF loading
- user fault delivery back to the owning task
- a general executable loader backed by storage services
- a richer VM syscall surface
- userspace-visible scheduler or signal policy
- kernel-mediated blocking receive completion for userspace threads
- package-backed manifest loading
- the broader platform-service graph
