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

## Phase 5

- reclaim boot-services memory safely
- install kernel-owned page tables
- build direct physical-memory mapping
- flesh out address-space creation APIs
- first real user address-space construction

## Phase 6

- first user thread launch path
- syscall ABI expansion around handles and objects
- root userspace service-manager handoff preparation

## Beyond

- root service manager
- platform services
- driver isolation strategy
- storage, networking, and graphics in services
- polished desktop stack
