# Roadmap

## Completed in Phase 1

- real UEFI memory-map capture
- early physical frame allocator
- initial kernel virtual layout
- x86_64 page-table mutation for heap mapping
- bootstrap kernel heap allocator
- kernel address-space root tracking

## Phase 2

- x86_64 GDT/TSS/IDT installation
- exception and fault classification
- PIC/PIT timer source integration
- monotonic time and deadline wakeup foundation
- syscall dispatch groundwork on vector `0x80`

## Completed in Phase 3

- registry-backed kernel object model
- bootstrap root task and per-task capability spaces
- handle rights, duplication, transfer, and close semantics
- channel IPC with message payloads and capability transfer
- object lifetime cleanup through weak registry tracking

## Completed in Phase 4

- bootstrap thread bring-up
- schedulable service threads
- round-robin scheduler foundation
- timer wakeup integration
- IPC receive blocking and wake integration
- task objects established as the current process-equivalent container

## Completed in Phase 5

- dedicated user page-table creation with shared kernel mappings
- minimal flat user image parsing and loading
- first ring-3 thread launch path
- bootstrap syscall ABI for ABI probe, time read, and thread exit
- first userspace demo program and return path back into the kernel

## Completed in Phase 6

- capability and IPC boundary hardening
- bounded IPC queue semantics
- explicit scheduler and capability exhaustion errors
- host-testable crate boundaries for freestanding targets
- unit coverage for object, capability, IPC, scheduler, syscall, memory, and
  userspace parsing invariants
- architecture summary and future-service readiness documentation

## Next

- user-visible handle and object syscalls
- channel and capability syscalls for real service composition
- user fault delivery instead of machine halt
- reclaim boot-services memory safely
- replace the bootstrap loader with a richer executable-loading path
- root userspace service-manager handoff preparation

## Beyond

- root service manager
- platform services
- driver isolation strategy
- storage, networking, and graphics in services
- polished desktop stack
