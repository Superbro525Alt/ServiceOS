# Boot Flow

## Default path

The default bring-up path uses:

- `cargo xtask run --platform qemu-virtio`
- a `platform/x86_64/qemu_virtio` boot parser for UEFI handoff
- `QEMU + OVMF` for the current fully working target
- a normalized `BootInfo` handoff into `kernel/core`

## Control flow

```text
QEMU
  -> OVMF / firmware
    -> EFI system partition
      -> platform/x86_64/qemu_virtio/image::BOOTX64.EFI
        -> kernel_main()
          -> early serial init
          -> read \serviceos\bootstore.bin from the EFI system partition
          -> capture ACPI RSDP pointer if present
          -> exit UEFI boot services
          -> platform/x86_64/qemu_virtio::boot::capture_boot_info()
          -> normalize UEFI memory map and boot-store bytes into BootInfo
          -> create arch/x86_64 active-page-table wrapper
          -> generic kernel memory initialization
          -> arch/x86_64 trap-table installation
          -> current x86 PC PIC remap and PIT programming
          -> register qemu-virtio display/input/network backends
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
- `BootInfo` population with usable, reclaimable, and reserved regions
- `BootInfo` transport of the staged boot store
- x86_64 active page-table access through the current CR3 root
- dedicated kernel heap mapping in the upper canonical half
- explicit reclaim of boot-services pages after heap bootstrap
- x86_64 descriptor-table installation before userspace handoff
- timer interrupt delivery through the legacy PIC/PIT path
- structured exception/fault reporting in Rust
- bootstrap root-task creation with an initial self capability
- registration of the bootstrap kernel thread before user handoff
- construction of a separate user page-table root for the root manager
- loading of flat userspace images and bootstrap user stacks from the boot store
- privilege transition into ring 3 and return through the syscall exit path
- launch of a persisted-manifest service graph after root-manager entry

## Secondary target bring-up

`cargo xtask image --platform raspi5` now stages a real Raspberry Pi 5 boot
image:

- `config.txt`
- `kernel8.img`
- `serviceos/bootstore.bin`
- `serviceos/serviceos-kernel.elf`
- an optional `bcm2712-rpi-5-b.dtb` copy when available locally
- a boot-partition directory layout under `target/images/<profile>/raspi5/boot`

The current control flow on that path is:

```text
Raspberry Pi firmware
  -> kernel8.img
    -> platform/aarch64/raspi5/image::_start
      -> AArch64 stack setup, BSS clear, and EL2 -> EL1 drop when needed
      -> DTB parse through platform/aarch64/raspi5::dtb
      -> normalize memory ranges into BootInfo
      -> resolve the chosen stdout UART from the DTB
      -> initialize PL011 debug UART
      -> build AArch64 page tables and enable the MMU
      -> install the EL1 exception vector
      -> initialize generic kernel memory, object, IPC, scheduler, and syscall state
      -> register UART-backed debug log and console hooks
      -> resolve userspace images from the embedded boot-store
      -> create the root userspace task and bootstrap channel
      -> enter EL0
      -> root manager starts the serial-first foundational service graph
      -> shell-service opens the serial console session
```

What is still deferred on that path:

- Raspberry Pi framebuffer backend
- Raspberry Pi input backend beyond the debug UART console path
- Raspberry Pi networking backend
- writable storage or boot-store update path on Raspberry Pi

## What is intentionally deferred

- switching to fully kernel-owned page tables
- direct-map installation for all physical memory
- fast `SYSCALL/SYSRET`
- general executable loading beyond the boot-store bootstrap path
