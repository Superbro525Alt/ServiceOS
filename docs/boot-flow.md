# Boot Flow

## Default path

The default bring-up path uses:

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
          -> read \serviceos\bootstore.bin from the EFI system partition
          -> capture ACPI RSDP pointer if present
          -> exit UEFI boot services
          -> normalize UEFI memory map and boot-store bytes into BootContext
          -> create x86_64 active-page-table wrapper
          -> generic kernel memory initialization
          -> x86_64 GDT/TSS/IDT installation
          -> PIC remap and PIT programming
          -> generic object, IPC, scheduler, and syscall initialization
          -> create the root userspace bootstrap channel and boot-store object
          -> create the root userspace task and user thread
          -> build a dedicated user address space
          -> load the root manager flat image from the boot store
          -> enter ring 3
          -> root manager starts storage-service
          -> root manager loads manifests from storage-service
          -> root manager starts the platform service graph
```

## What is implemented now

- real UEFI memory-map capture after `ExitBootServices`
- boot-store file loading before `ExitBootServices`
- boot context population with usable, reclaimable, and reserved regions
- boot context transport of the staged boot store
- x86_64 active page-table access through the current CR3 root
- dedicated kernel heap mapping in the upper canonical half
- x86_64 descriptor-table installation before `sti`
- timer interrupt delivery through the legacy PIC/PIT path
- structured exception/fault reporting in Rust
- bootstrap root-task creation with an initial self capability
- registration of the bootstrap kernel thread before user handoff
- construction of a separate user page-table root for the root manager
- loading of flat userspace images and bootstrap user stacks from the boot store
- privilege transition into ring 3 and return through the syscall exit path
- launch of a persisted-manifest service graph after root-manager entry

## What is intentionally deferred

- switching to fully kernel-owned page tables
- reclaiming boot-services memory
- direct-map installation for all physical memory
- fast `SYSCALL/SYSRET`
- general executable loading beyond the boot-store bootstrap path
