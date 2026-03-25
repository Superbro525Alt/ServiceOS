# Later Phases

## Phase 1

- physical memory manager
- kernel virtual address layout
- early page management
- interrupt and exception table installation
- basic timer source

## Phase 2

- kernel object registry
- capability spaces
- address spaces and task containers
- first syscall dispatch layer

## Phase 3

- thread model
- scheduler scaffolding
- IPC endpoints and message transfer model
- root userspace task launch

## Beyond

- root service manager
- platform services
- driver isolation strategy
- storage, networking, and graphics in services
- polished desktop stack

Each phase should add mechanisms only after the module boundaries already exist
to host them cleanly.

