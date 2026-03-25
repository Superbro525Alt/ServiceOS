# Boot Flow

## Default path

Phase 2 uses:

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
          -> x86_64 GDT/TSS/IDT installation
          -> PIC remap and PIT programming
          -> enable interrupts
          -> wait for timer-driven wakeup
          -> halt loop
```

## What is implemented now

- real UEFI memory-map capture after `ExitBootServices`
- boot context population with usable, reclaimable, and reserved regions
- x86_64 active page-table access through the current CR3 root
- dedicated kernel heap mapping in the upper canonical half
- x86_64 descriptor-table installation before `sti`
- timer interrupt delivery through the legacy PIC/PIT path
- deadline wakeup processing in generic kernel time code
- structured exception/fault reporting in Rust

## What is intentionally deferred

- switching to fully kernel-owned page tables
- reclaiming boot-services memory
- direct-map installation for all physical memory
- fast syscall instructions and ring-3 transition support
- userspace launch
