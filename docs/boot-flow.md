# Boot Flow

## Default path

Phase 5 uses:

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
          -> generic object and IPC initialization
          -> bootstrap thread creation and scheduler initialization
          -> create first service task with a user thread
          -> build a dedicated user address space
          -> load the flat bootstrap user image
          -> enter ring 3
          -> service minimal syscalls from the first user program
          -> return to the bootstrap kernel thread
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
- bootstrap root-task creation with an initial self capability
- a registry-backed object model initialized before interrupts are enabled
- bootstrap thread registration before the first user handoff
- construction of a separate user page-table root for the first service task
- loading of a minimal flat user image and bootstrap user stack
- privilege transition into ring 3 and return through the syscall exit path
- a boot-time self-check that validates user launch, syscall entry, monotonic
  time reads, and user-thread exit

## What is intentionally deferred

- switching to fully kernel-owned page tables
- reclaiming boot-services memory
- direct-map installation for all physical memory
- fast `SYSCALL/SYSRET`
- general executable loading
- the real root service manager
