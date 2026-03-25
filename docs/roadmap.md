# Roadmap

## Completed in Phase 1

- real UEFI memory-map capture
- early physical frame allocator
- initial kernel virtual layout
- x86_64 page-table mutation for heap mapping
- bootstrap kernel heap allocator
- kernel address-space root tracking

## Phase 2

- reclaim boot-services memory safely
- install kernel-owned page tables
- build direct physical-memory mapping
- flesh out address-space creation APIs
- begin kernel object allocation strategy beyond the bootstrap heap

## Phase 3

- capability spaces
- syscall entry and dispatch
- thread and task bring-up
- first user address-space construction

## Beyond

- root service manager
- platform services
- driver isolation strategy
- storage, networking, and graphics in services
- polished desktop stack
