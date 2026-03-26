# Roadmap

## Kernel foundation completed

- firmware handoff, memory discovery, paging, and kernel heap bootstrap
- interrupt, exception, syscall, and timer foundations
- kernel object model, capabilities, and IPC
- scheduler, task model, and userspace entry
- hardening, tests, and architecture cleanup

## Root bootstrap completed

- root userspace service manager
- persisted boot-store manifest index
- dependency-ordered startup
- capability-scoped startup grants
- manager-mediated registration and discovery

## Foundational services completed

- `storage-service`
- `console-service`
- `config-service`
- `log-service`
- `status-service`
- `network-service`
- explicit lookup policy and per-handle transfer-right control

## Platform tooling completed

- `shell-service` operator environment
- `package-service` install, update, rollback, and activation coordination

## Next

- reclaim boot-services memory safely
- evolve the boot-store bootstrap into richer executable-loading policy
- add writable storage and directory capabilities
- extend terminate-on-fault isolation into richer user-fault upcalls and
  recovery policy
- move packet I/O from timer polling to device-driven wakeups
- add dynamic IPv4 configuration and DNS-backed resolution behind the current
  network-service contract
- grow the current ICMP/status path into a broader socket and transport surface

## Later

- storage/filesystem services
- graphics/compositor services
- compatibility runtimes
- desktop/session layers
