# Boot Flow

## Default path

Phase 1 uses:

- the `uefi` crate in the kernel image crate for firmware entry
- the host-side `xtask` tool to stage an EFI system partition
- `QEMU + OVMF` for the default run target

## Control flow

```text
QEMU
  -> OVMF / firmware
    -> EFI system partition
      -> kernel/image/x86_64::BOOTX64.EFI
        -> kernel_main()
          -> early serial init
          -> capture ACPI RSDP pointer if present
          -> exit UEFI boot services
          -> normalize UEFI memory map into BootContext
          -> create x86_64 active-page-table wrapper
          -> generic kernel memory initialization
          -> halt loop
```

## What is implemented now

- real UEFI memory-map capture after `ExitBootServices`
- boot context population with usable, reclaimable, and reserved regions
- x86_64 active page-table access through the current CR3 root
- dedicated kernel heap mapping in the upper canonical half

## What is intentionally deferred

- switching to fully kernel-owned page tables
- reclaiming boot-services memory
- direct-map installation for all physical memory
- interrupt table install and timer bring-up
- userspace launch
