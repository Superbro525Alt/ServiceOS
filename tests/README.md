# Test Strategy

Phase 0 keeps verification intentionally small:

- `cargo fmt --check`
- `cargo check --workspace`
- `cargo xtask build`
- `timeout 10 cargo xtask qemu`

Later phases should add:

- host-side model tests for parsing and data normalization
- boot smoke tests under QEMU
- subsystem unit tests where `no_std` boundaries allow them
- integration tests for syscall and IPC behavior once userspace exists
