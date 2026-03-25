# Userspace Bootstrap

## Phase 5 scope

Phase 5 adds the first real kernel-to-userspace handoff.

The kernel now:

- creates a dedicated user page-table root
- maps a minimal executable image and a bootstrap stack
- creates a `ThreadMode::User` thread owned by a service task
- enters ring 3 on `x86_64`
- handles a minimal syscall round trip
- returns to the kernel when the user thread exits

This is a bootstrap path, not a full process runtime.

## Executable format

The first userspace image is a deliberately small flat format owned by
`kernel/core::user`.

The header carries:

- a magic value
- an ABI version
- an image base
- an entry offset
- a code byte count
- a user stack top

Why a flat image instead of ELF now:

- it keeps Phase 5 focused on privilege transition and address-space ownership
- it avoids dragging relocation, segment, and filesystem policy into the kernel
- it gives later phases a replaceable loader boundary instead of a one-off blob

## Address-space model

The Phase 5 user address space is built by:

- allocating a new top-level page table
- copying the kernel-visible mappings needed for kernel entry and return
- mapping one user executable region
- mapping one bootstrap user stack region

The current loader does not yet expose a general VM API to userspace. It is a
kernel-owned construction path for the first program only.

## Initial syscall ABI

The first user program uses interrupt vector `0x80`.

Current syscall numbers:

- `0`: return the kernel syscall ABI version
- `1`: return the current monotonic tick count
- `2`: terminate the current thread with a supplied status code

The ABI is intentionally small:

- no handle table syscalls yet
- no user buffer marshalling yet
- no memory mapping syscalls yet
- no IPC syscalls exposed to ring 3 yet

That keeps the kernel/userspace contract narrow while the first launch path is
still stabilizing.

## What the demo proves

The demo userspace image proves that the kernel can:

- transfer from firmware-driven kernel bring-up into a user-controlled
  instruction stream
- execute with user CS/SS selectors and a user stack
- trap back into the kernel through the syscall path
- resume kernel execution after the user thread exits

This is the minimum needed before a real root service manager becomes credible.

## Still deferred

Phase 5 does not yet include:

- ELF loading
- user-visible handle, IPC, or VM syscalls
- user fault delivery back to the owning task
- process spawning policy
- executable discovery or filesystem-backed loading
- the real root service manager
