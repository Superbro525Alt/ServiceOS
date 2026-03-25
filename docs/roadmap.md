# Roadmap

## Kernel foundation completed

- firmware handoff, memory discovery, paging, and kernel heap bootstrap
- interrupt, exception, syscall, and timer foundations
- kernel object model, capabilities, and IPC
- scheduler, task model, and userspace entry
- hardening, tests, and architecture cleanup

## Root bootstrap completed

- root userspace service manager
- built-in manifest catalog
- dependency-ordered startup
- capability-scoped startup grants
- manager-mediated registration and discovery

## Foundational services completed

- `console-service`
- `config-service`
- `log-service`
- `status-service`
- explicit lookup policy and per-handle transfer-right control

## Next

- replace the temporary bootstrap-root role gate with an explicit bootstrap
  capability object
- add user fault delivery instead of machine halt
- integrate real kernel-backed blocking receive completion for userspace waits
- reclaim boot-services memory safely
- replace the built-in flat-image catalog with a richer executable-loading path
- grow the manifest source beyond compiled-in data

## Later

- storage/filesystem services
- networking services
- package and update services
- shell/session services
- graphics/compositor services
- compatibility runtimes
