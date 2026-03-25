# Roadmap

## Completed in Phase 1

- real UEFI memory-map capture
- early physical frame allocator
- initial kernel virtual layout
- x86_64 page-table mutation for heap mapping
- bootstrap kernel heap allocator
- kernel address-space root tracking

## Completed in Phase 2

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

## Completed in Root Userspace Bootstrap

- shared kernel/userspace ABI crate
- built-in flat-image catalog for freestanding userspace services
- first real root service manager launched by the kernel
- manifest-driven dependency startup in userspace
- capability-scoped startup grants from root to child services
- manager-mediated service registration and discovery
- restart supervision for a one-shot bootstrap validator
- example log, echo, and probe services

## Next

- replace the temporary bootstrap-root role gate with an explicit bootstrap
  capability object
- add user fault delivery instead of machine halt
- integrate real kernel-backed blocking receive completion for userspace waits
- reclaim boot-services memory safely
- replace the built-in flat-image catalog with a richer executable-loading path
- grow the service manifest source beyond compiled-in data

## Beyond

- platform services
- driver isolation strategy
- storage, networking, and graphics in services
- polished desktop stack
