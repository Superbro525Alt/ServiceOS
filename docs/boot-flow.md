# Boot Flow

## Default path

Phase 0 uses:

- the `uefi` crate in the kernel image crate for the firmware entry contract
- the host-side `xtask` tool to stage an EFI system partition directory
- `QEMU + OVMF` for the default UEFI execution path

## Control flow

```text
QEMU
  -> OVMF / firmware
    -> EFI system partition
      -> kernel/image/x86_64::BOOTX64.EFI
        -> kernel_main()
          -> arch x86_64 early bring-up
          -> boot context synthesis
          -> kernel/core initialization boundary
          -> halt loop
```

## What Phase 0 does not do

- no memory allocator
- no page-table manager
- no scheduler
- no userspace launch
- no syscall ABI implementation
- no real interrupt descriptor setup
- no full UEFI memory-map normalization yet

The boot path exists only to define the early control boundary and to prove that
the repository structure can carry a real kernel later.
