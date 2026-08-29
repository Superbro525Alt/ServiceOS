# qemu-isa handoff — final userspace-entry fault

> STATUS UPDATE (latest session): the syscall-return/eret defect described
> below was fixed on the aarch64 path via a memory result-slot protocol
> (kernel publishes (value,error) to [sp_el0-16,sp_el0-8]; runtime reads
> post-svc from that slot — see commit 38b8463). The SAME root cause likely
> explains the qemu-isa GPF at first user entry. qemu-isa is untested since;
> next agent should retry a boot, and if the GPF persists, port the
> result-slot protocol to the x86_64 syscall path.

## What works (verified)
- `cargo xtask run --platform qemu-isa` boots via SeaBIOS PVH ELF note.
- Long-mode trampoline (`mb_entry.S`): identity 2MiB pages, tables at
  0x30000/0x31000/0x32000 (must avoid ELF LOAD PhysAddr=0x7000!), NXE set
  with LME, EBX preserved through fill loop.
- QEMU PVH v1 start_info has an extra u64 @32 before memmap_paddr; magic is
  0x336EC578. memmap stride-16 entries parsed into BootInfo regions.
- Kernel init fully succeeds on BIOS path: heap, LAPIC timer calibrated,
  kthread ping-pong 1000/1000 switches, HPET, SMP probe, PIC/PIT.
- Bootstrap reaches `entering userspace executor`.

## The remaining bug
- First `resume_user` IRETQ raises #GP(0xff50). QEMU: pc = the IRETQ inside
  `serviceos_x86_64_resume_user`; restored RAX =
  0xf000ff53f000ff53 — exact BIOS IVT qword pattern. So the SavedUserContext
  bytes contain stale low-memory BIOS data at restore time.
- Same code path reaches desktop-ready under qemu-virtio (UEFI).
- TEMP-DEBUG(qisa-gpf) breadcrumbs exist in arch/x86_64/src/user.rs
  (~line 508-560, statics serviceos_kdbg_*) — they did NOT print before the
  GPF (reset happens first); remove after root cause.

## Leading hypotheses (unproven)
1. The SavedUserContext handed to resume_user lives in memory that gets
   clobbered between spawn-time `initial()` write and executor resume —
   e.g., context stored on run_thread's kernel stack frame that timer/kthread
   activity overwrites, or USER_THREADS slot index mismatch after the
   kthread demo shifted thread ids (works on UEFI by timing luck).
2. BIOS-path frame pool hands out a low (<1MiB) frame for the context —
   BootInfo marks <1MiB BootloaderOwned, but check zero-length entries and
   any separate low pool.
3. Double registration: register_thread_launch writes initial ctx, then a
   second path overwrites with a stale pointer.

## Debug entry points
- Fault site: arch/x86_64/src/user.rs `serviceos_x86_64_resume_user`
  (iretq at end; symbol near 0x121541 in current build).
- Executor: platform/x86_64/qemu_virtio/image/src/executor.rs (USER_THREADS).
- Compare UEFI run state at same logical point: QEMU_HEADLESS=1 cargo xtask
  run --platform qemu-virtio + `-d int -D log`.
- Useful tracing: QEMU_EXTRA_ARGS="-d int -D /tmp/qi.log" then grep " v=0d ".

## Also unfinished (older, known)
- userspace/programs/runtime/src/lib.rs LSP shows duplicate panic_impl under
  host LSP — cosmetic (host-target check), real targets build.
- Concurrent-session WIP in userspace/* was mid-refactor at snapshot time;
  workspace compiled green at commit time except their newest churn.
