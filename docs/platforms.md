# Architecture And Platform Split

## Layering

The repository now trends toward three layers:

- `kernel/core`
- `arch/<isa>`
- `platform/<platform>`

The split is intentional:

- `kernel/core` owns kernel semantics and backend-neutral contracts
- `arch/<isa>` owns CPU privilege, MMU, trap, syscall, and user-transition
  mechanics
- `platform/<platform>` owns firmware parsing and board or device wiring

## Current targets

### `qemu-virtio`

- ISA: `x86_64`
- Arch crate: `arch/x86_64`
- Platform crate: `platform/x86_64/qemu_virtio`
- Boot model: UEFI + OVMF
- Run model: `cargo xtask run --platform qemu-virtio`

Implemented today:

- UEFI boot parsing into `BootInfo`
- serial console
- boot framebuffer display backend
- VirtIO input backend
- VirtIO PCI network backend
- bootable disk-image generation and QEMU launch

### `raspi5`

- ISA: `aarch64`
- Arch crate: `arch/aarch64`
- Platform crate: `platform/aarch64/raspi5`
- Boot model: Raspberry Pi firmware
- Run model: native boot-image staging and manual deployment

Implemented today:

- first-class workspace target and crate layout
- native `platform/aarch64/raspi5/image` kernel image crate
- `xtask` platform selection and raw `kernel8.img` generation
- Raspberry Pi firmware boot-partition staging with `config.txt`
- DTB-backed memory discovery and stdout-UART resolution
- PL011 debug-UART logging after native AArch64 entry

Still deferred:

- full generic-kernel initialization on `aarch64`
- real `aarch64` exception, MMU, syscall, and user-transition code
- Raspberry Pi display, input, networking, and storage backends
- userspace service bootstrap on Raspberry Pi 5

## Xtask model

`xtask` is now platform-first.

Working commands:

```bash
cargo xtask build --platform qemu-virtio
cargo xtask run --platform qemu-virtio
cargo xtask image --platform raspi5
```

Compatibility alias:

```bash
cargo xtask qemu
```

Current behavior:

- `qemu-virtio` builds the x86_64 arch crate, the QEMU/VirtIO platform crate,
  the UEFI kernel image, the userspace catalog, and then creates a raw disk
  image
- `raspi5` builds the aarch64 arch crate, the Raspberry Pi 5 platform crate,
  the native Raspberry Pi image crate, the userspace catalog, and stages a Pi
  boot-partition bundle with a real `kernel8.img`

## Temporary but explicit gaps

- parts of the current x86 PC interrupt-controller path still live in
  `arch/x86_64`, because there is only one x86 platform target today
- x86 trap-time emergency serial output is still local to `arch/x86_64`; the
  main serial backend is already under `platform/x86_64/qemu_virtio`
- the Raspberry Pi 5 target is a real native bring-up image, not a complete
  ServiceOS hardware port yet
