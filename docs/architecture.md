# Kernel Architecture

## Philosophy

The kernel exists to provide mechanisms that let later userspace services build
policy.

The long-term operating system model is:

```text
kernel
  -> root service manager
    -> foundational services
      -> platform services
        -> shells, runtimes, applications, compatibility layers
```

Phase 0 deliberately implements none of those userspace layers. It only creates
the kernel foundation that lets them appear later without redesigning the
repository or boot path.

## Kernel responsibilities

The kernel will eventually own:

- virtual and physical memory mechanisms
- task and address-space mechanisms
- interrupt, exception, and timer control
- capability and kernel object mediation
- IPC transport and syscall entry

The kernel will explicitly avoid embedding high-level service policy such as:

- filesystem semantics
- networking stacks
- device management policy
- GUI and desktop composition
- application runtime policy
- system configuration policy

## Design direction

- Small kernel, higher-level functionality in services
- Capability-oriented object access instead of ambient authority
- Strong separation between generic subsystems and architecture code
- Boot-time code kept narrow so later firmware or loader changes do not reshape
  the rest of the kernel
- Repository organized for long-term subsystem ownership, not tutorial-style
  bring-up

## Phase 0 decisions

- `x86_64` is the first supported architecture
- QEMU is the primary development target
- UEFI is the default firmware path for QEMU bring-up
- The initial boot path is a direct UEFI application entry so repository
  structure and subsystem boundaries can settle before a more advanced loader
  contract is introduced
