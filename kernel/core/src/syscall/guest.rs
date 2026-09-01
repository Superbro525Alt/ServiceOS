//! Guest syscall-ABI dispatch: live translation path for tasks spawned with
//! the `linux-syscall` ABI flag.
//!
//! Hooked at the top of the syscall dispatch (see `interrupts::dispatch_syscall`)
//! BEFORE the native numbering so a `linux-syscall` task's `int 0x80` trap is
//! translated through `serviceos_abi::linux_abi` instead of colliding with
//! ServiceOS numbers (Linux write=1 would otherwise hit `MonotonicNow`, Linux
//! mmap=9 would hit `HandleClose`, Linux exit=60 is an unallocated slot).
//!
//! Executed families (bounded scope, everything else stays `-ENOSYS`):
//! - `write` → `DebugConsoleWrite` (fd 1/2 only; arguments re-marshaled)
//! - `exit` / `exit_group` → `ThreadExit` (status passthrough)
//! - `clock_gettime` (CLOCK_MONOTONIC) / `gettimeofday` → `MonotonicNow`
//!   converted into the guest timespec/timeval layout
//!
//! Guest result encoding: `rax = value` on success, `rax = -(errno)` on
//! failure, `rdx = 0` in both cases (the Linux convention; native tasks keep
//! the `rdx` error slot).
//!
//! Entry-path reality per architecture: x86_64 guests trap through the
//! `int 0x80` gate with the Linux number in `rax`; aarch64 guests trap
//! through `svc #0` with the Linux number in `w8`. The native ServiceOS
//! aarch64 entry already reads the number from `x8`, so a Linux-mode
//! aarch64 task's numbers reach the shared dispatcher unchanged and only
//! the table lookup differs. The kernel is compiled per-arch (`arch/*`):
//! table selection is a compile-time match below — x86_64 → the Linux
//! x86_64 table, aarch64 → the Linux ARM64 table, other architectures
//! (riscv64) have no guest ABI yet and resolve every call to `-ENOSYS` so
//! the hazard cannot silently mis-map.

use serviceos_abi::linux_abi::{self, GuestTimespec, GuestTimeval, errno, spawn_abi};

use super::{
    SyscallContext, SyscallNumber, SyscallReturn, handle_debug_console_write, handle_monotonic_now,
    handle_thread_exit,
};
use crate::user;

/// Syscall ABI a task enters syscalls through. Default is [`Self::Native`];
/// only explicitly flagged spawns get guest translation. The `Linux` mode
/// selects the Linux table matching the kernel's compile-time architecture
/// (see `dispatch_linux_syscall`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuestSyscallAbi {
    Native,
    Linux,
}

impl Default for GuestSyscallAbi {
    fn default() -> Self {
        Self::Native
    }
}

impl GuestSyscallAbi {
    /// Decode the additive `TaskSpawnImage` flag word. Unknown values are
    /// rejected so a typo cannot silently fall back to native numbering.
    pub const fn from_spawn_flags(flags: u64) -> Option<Self> {
        match flags {
            spawn_abi::NATIVE => Some(Self::Native),
            spawn_abi::LINUX_SYSCALL => Some(Self::Linux),
            _ => None,
        }
    }
}

/// Translate-and-execute entry. `None` = keep native dispatch (task is
/// native, or no current task context exists).
pub fn dispatch_guest(number: SyscallNumber, context: &SyscallContext) -> Option<SyscallReturn> {
    let abi = user::current_task_syscall_abi()?;
    match abi {
        GuestSyscallAbi::Native => None,
        GuestSyscallAbi::Linux => Some(dispatch_linux_syscall(number.0, context)),
    }
}

/// Compile-time table selection: the kernel is compiled per-arch (`arch/*`),
/// so the arch that compiles this module is the arch the task traps through.
/// x86_64 picks the Linux x86_64 table (`int 0x80`, number in `rax`),
/// aarch64 the Linux ARM64 table (`svc #0`, number in `w8` — the same
/// register the native aarch64 entry reads), everything else stays ENOSYS.
fn dispatch_linux_syscall(linux_number: u32, context: &SyscallContext) -> SyscallReturn {
    #[cfg(target_arch = "x86_64")]
    {
        dispatch_linux_x64(linux_number, context)
    }
    #[cfg(target_arch = "aarch64")]
    {
        dispatch_linux_arm64(linux_number, context)
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        let _ = (linux_number, context);
        linux_errno_error(errno::ENOSYS)
    }
}

/// x86_64 translate-and-execute twin. Compiled on every target so host
/// tests can exercise the full matrix; silenced where the live path
/// selects the ARM64 twin instead.
#[cfg_attr(not(any(target_arch = "x86_64", test)), allow(dead_code))]
fn dispatch_linux_x64(linux_number: u32, context: &SyscallContext) -> SyscallReturn {
    if linux_abi::translate_syscall(linux_number) == linux_abi::TRANSLATE_ENOSYS {
        return linux_errno_error(errno::ENOSYS);
    }
    match linux_number {
        linux_abi::linux::WRITE => guest_write(context),
        linux_abi::linux::EXIT | linux_abi::linux::EXIT_GROUP => {
            // Status passthrough: the guest's exit code becomes the task
            // exit code, exactly as for native ThreadExit.
            handle_thread_exit(context)
        }
        linux_abi::linux::CLOCK_GETTIME => guest_clock_gettime(context),
        linux_abi::linux::GETTIMEOFDAY => guest_gettimeofday(context),
        // Decided table rows without an executable kernel path yet
        // (read/close/dup/dup2/mmap family/yield) stay ENOSYS by scope.
        _ => linux_errno_error(errno::ENOSYS),
    }
}

/// ARM64 translate-and-execute twin: same shape and family scope as the
/// x86_64 twin, keyed by the Linux ARM64 numbering. Register conventions
/// match the native aarch64 entry (number in x8), so translation is purely
/// a table lookup — no entry-path divergence.
#[cfg_attr(not(any(target_arch = "aarch64", test)), allow(dead_code))]
fn dispatch_linux_arm64(linux_number: u32, context: &SyscallContext) -> SyscallReturn {
    if linux_abi::translate_arm64_syscall(linux_number) == linux_abi::TRANSLATE_ENOSYS {
        return linux_errno_error(errno::ENOSYS);
    }
    match linux_number {
        linux_abi::linux_arm64::WRITE => guest_write(context),
        linux_abi::linux_arm64::EXIT | linux_abi::linux_arm64::EXIT_GROUP => {
            // Status passthrough: the guest's exit code becomes the task
            // exit code, exactly as for native ThreadExit.
            handle_thread_exit(context)
        }
        linux_abi::linux_arm64::CLOCK_GETTIME => guest_clock_gettime(context),
        linux_abi::linux_arm64::GETTIMEOFDAY => guest_gettimeofday(context),
        // Decided table rows without an executable kernel path yet
        // (read/close/dup/mmap family/yield) stay ENOSYS by scope.
        _ => linux_errno_error(errno::ENOSYS),
    }
}

/// Linux `write(fd, buf, count)` → console-scoped `DebugConsoleWrite`.
/// Argument slots shift: Linux `[fd, buf, count]` → native `[buf, len]`.
fn guest_write(context: &SyscallContext) -> SyscallReturn {
    match context.arguments[0] {
        1 | 2 => {}
        _ => return linux_errno_error(errno::EBADF),
    }
    let native = SyscallContext {
        instruction_pointer: context.instruction_pointer,
        stack_pointer: context.stack_pointer,
        flags: context.flags,
        arguments: [context.arguments[1], context.arguments[2], 0, 0, 0, 0],
    };
    to_guest_result(handle_debug_console_write(&native))
}

/// Linux `clock_gettime(CLOCK_MONOTONIC, tp)` → monotonic ticks converted
/// into the guest timespec layout.
fn guest_clock_gettime(context: &SyscallContext) -> SyscallReturn {
    if context.arguments[0] != linux_abi::clock::CLOCK_MONOTONIC {
        return linux_errno_error(errno::EINVAL);
    }
    if context.arguments[1] == 0 {
        // Linux demands a timespec buffer here; refuse instead of guessing.
        return linux_errno_error(errno::EFAULT);
    }
    let Some(manager) = crate::time::manager() else {
        return linux_errno_error(errno::ENOSYS);
    };
    let (sec, nsec) = linux_abi::ticks_to_timespec(monotonic_tick(), manager.source().tick_hz);
    match unsafe { super::user_mut::<GuestTimespec>(context.arguments[1]) } {
        Ok(timespec_out) => {
            *timespec_out = GuestTimespec {
                tv_sec: sec,
                tv_nsec: nsec,
            };
            SyscallReturn::success(0)
        }
        Err(_) => linux_errno_error(errno::EFAULT),
    }
}

/// Linux `gettimeofday(tv, tz)` → monotonic ticks converted into the guest
/// timeval layout; the legacy `tz` argument is ignored.
fn guest_gettimeofday(context: &SyscallContext) -> SyscallReturn {
    if context.arguments[0] == 0 {
        return linux_errno_error(errno::EFAULT);
    }
    let Some(manager) = crate::time::manager() else {
        return linux_errno_error(errno::ENOSYS);
    };
    let (sec, usec) = linux_abi::ticks_to_timeval(monotonic_tick(), manager.source().tick_hz);
    match unsafe { super::user_mut::<GuestTimeval>(context.arguments[0]) } {
        Ok(timeval_out) => {
            *timeval_out = GuestTimeval {
                tv_sec: sec,
                tv_usec: usec,
            };
            SyscallReturn::success(0)
        }
        Err(_) => linux_errno_error(errno::EFAULT),
    }
}

fn monotonic_tick() -> u64 {
    handle_monotonic_now(&SyscallContext {
        instruction_pointer: 0,
        stack_pointer: 0,
        flags: 0,
        arguments: [0; 6],
    })
    .value
}

/// Convert a native `SyscallReturn` into the guest ABI encoding:
/// success keeps the value, failure becomes `rax = -(errno)`, `rdx = 0`.
/// Syscall actions (yield/exit/block) pass through untouched.
fn to_guest_result(result: SyscallReturn) -> SyscallReturn {
    match result.error {
        None => result,
        Some(_) => {
            let errno = linux_abi::errno_for_serviceos_error(result.abi_error_code());
            SyscallReturn {
                value: linux_abi::error_result(errno),
                error: None,
                action: result.action,
            }
        }
    }
}

fn linux_errno_error(code: u64) -> SyscallReturn {
    SyscallReturn::success(linux_abi::error_result(code))
}

#[cfg(test)]
mod tests {
    use super::super::SyscallError;
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};

    fn context(args: [u64; 6]) -> SyscallContext {
        SyscallContext {
            instruction_pointer: 0x1000,
            stack_pointer: 0x7fff_0000,
            flags: 0x202,
            arguments: args,
        }
    }

    #[test]
    fn spawn_flag_words_decode_strictly() {
        assert_eq!(
            GuestSyscallAbi::from_spawn_flags(spawn_abi::NATIVE),
            Some(GuestSyscallAbi::Native)
        );
        assert_eq!(
            GuestSyscallAbi::from_spawn_flags(spawn_abi::LINUX_SYSCALL),
            Some(GuestSyscallAbi::Linux)
        );
        assert_eq!(GuestSyscallAbi::from_spawn_flags(1), None);
        assert_eq!(GuestSyscallAbi::from_spawn_flags(42), None);
        assert_eq!(
            GuestSyscallAbi::from_spawn_flags(spawn_abi::LINUX_SYSCALL - 1),
            None
        );
        // The Linux mode stays one flag word for every guest architecture;
        // the table is chosen by the kernel's compile-time arch.
        assert_ne!(GuestSyscallAbi::Native, GuestSyscallAbi::Linux);
    }

    #[test]
    fn uninitialized_runtime_leaves_native_dispatch_untouched() {
        // No user runtime in a host test: dispatch must report None so the
        // native numbering passes through unchanged (flag-off contract).
        assert_eq!(dispatch_guest(SyscallNumber(60), &context([0; 6])), None);
        assert_eq!(dispatch_guest(SyscallNumber(14), &context([0; 6])), None);
    }

    #[test]
    fn guest_failures_encode_as_negated_errno_without_error_slot() {
        let result = to_guest_result(SyscallReturn::error(SyscallError::NotInitialized));
        assert_eq!(result.value, (-38i64) as u64);
        assert_eq!(result.error, None);

        let result = to_guest_result(SyscallReturn::error(SyscallError::InvalidArgument));
        assert_eq!(result.value, (-22i64) as u64);
        assert_eq!(result.error, None);

        // Success values pass through untouched.
        let result = to_guest_result(SyscallReturn::success(17));
        assert_eq!(result.value, 17);
        assert_eq!(result.error, None);
    }

    #[test]
    fn exit_action_passes_through_guest_encoding() {
        let result = to_guest_result(SyscallReturn::exit_current_thread(42));
        assert_eq!(result.value, 42);
        assert_eq!(result.error, None);
        assert!(matches!(
            result.action,
            super::super::SyscallAction::ExitCurrentThread { status: 42 }
        ));
    }

    #[test]
    fn untranslatable_linux_numbers_stay_outside_the_table() {
        // open(2), fork(57), socket(41): no decided row, never dispatched.
        assert_eq!(linux_abi::translate_syscall(2), linux_abi::TRANSLATE_ENOSYS);
        assert_eq!(
            linux_abi::translate_syscall(57),
            linux_abi::TRANSLATE_ENOSYS
        );
        assert_eq!(
            linux_abi::translate_syscall(41),
            linux_abi::TRANSLATE_ENOSYS
        );
    }

    #[test]
    fn address_space_abi_map_defaults_native_and_round_trips() {
        let runtime = crate::user::initialize_runtime();
        let id = runtime.allocate_address_space_id();
        assert_eq!(runtime.syscall_abi(id), GuestSyscallAbi::Native);
        runtime.set_syscall_abi(id, GuestSyscallAbi::Linux);
        assert_eq!(runtime.syscall_abi(id), GuestSyscallAbi::Linux);
        // Unlisted address spaces stay native (flag-off contract).
        let other = crate::task::AddressSpaceId(id.0.wrapping_add(9999));
        assert_eq!(runtime.syscall_abi(other), GuestSyscallAbi::Native);
    }

    #[test]
    fn abi_helper_demands_a_current_task() {
        // Host test: runtime may exist but no task is running, so the
        // helper must report None (dispatch stays native) rather than
        // inventing a guest mode.
        let _ = crate::user::initialize_runtime();
        assert_eq!(user::current_task_syscall_abi(), None);
    }

    #[test]
    fn linux_dispatch_matrix_executes_decided_families_on_both_arches() {
        static WRITE_CALLS: AtomicUsize = AtomicUsize::new(0);
        static WRITE_BYTES: AtomicUsize = AtomicUsize::new(0);
        fn probe_writer(bytes: &[u8]) {
            WRITE_CALLS.fetch_add(1, Ordering::SeqCst);
            WRITE_BYTES.fetch_add(bytes.len(), Ordering::SeqCst);
        }
        crate::syscall::register_debug_console_writer(probe_writer);

        // 1 kHz tick source; advance the shared manager to 2500 ticks = 2.5 s.
        let _ = crate::time::initialize(crate::time::TimerSourceInfo { tick_hz: 1000 });
        let manager = crate::time::manager().expect("time manager");
        for _ in 0..2500 {
            manager.handle_tick();
        }

        // write(1, buf, 8): remaps to native [buf, 8], executes, returns count.
        let buffer = alloc::vec![0xABu8; 8];
        let buffer_ptr = buffer.as_ptr() as u64;
        let result = dispatch_linux_x64(
            linux_abi::linux::WRITE,
            &context([1, buffer_ptr, 8, 0, 0, 0]),
        );
        assert_eq!(result.value, 8);
        assert_eq!(result.error, None);
        assert_eq!(WRITE_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(WRITE_BYTES.load(Ordering::SeqCst), 8);

        // write(2, ...) is stderr: same console path.
        let result = dispatch_linux_x64(
            linux_abi::linux::WRITE,
            &context([2, buffer_ptr, 0, 0, 0, 0]),
        );
        assert_eq!(result.value, 0);
        assert_eq!(WRITE_CALLS.load(Ordering::SeqCst), 2);

        // write on an unsupported fd: -EBADF, console untouched.
        let result = dispatch_linux_x64(
            linux_abi::linux::WRITE,
            &context([7, buffer_ptr, 4, 0, 0, 0]),
        );
        assert_eq!(result.value, (-9i64) as u64);
        assert_eq!(WRITE_CALLS.load(Ordering::SeqCst), 2);

        // exit / exit_group: status passthrough as ExitCurrentThread.
        let result = dispatch_linux_x64(linux_abi::linux::EXIT, &context([42, 0, 0, 0, 0, 0]));
        assert!(matches!(
            result.action,
            super::super::SyscallAction::ExitCurrentThread { status: 42 }
        ));
        let result = dispatch_linux_x64(linux_abi::linux::EXIT_GROUP, &context([7, 0, 0, 0, 0, 0]));
        assert!(matches!(
            result.action,
            super::super::SyscallAction::ExitCurrentThread { status: 7 }
        ));

        // clock_gettime(CLOCK_MONOTONIC, tp): real timespec written to a
        // heap-backed buffer (host test heap addresses sit inside the user
        // pointer window).
        let mut timespec = GuestTimespec {
            tv_sec: -1,
            tv_nsec: -1,
        };
        let timespec_ptr = &mut timespec as *mut GuestTimespec as u64;
        let result = dispatch_linux_x64(
            linux_abi::linux::CLOCK_GETTIME,
            &context([linux_abi::clock::CLOCK_MONOTONIC, timespec_ptr, 0, 0, 0, 0]),
        );
        assert_eq!(result.value, 0);
        assert_eq!(timespec.tv_sec, 2);
        assert_eq!(timespec.tv_nsec, 500_000_000);

        // clock_gettime(CLOCK_REALTIME): unsupported clock id.
        let result = dispatch_linux_x64(
            linux_abi::linux::CLOCK_GETTIME,
            &context([0, timespec_ptr, 0, 0, 0, 0]),
        );
        assert_eq!(result.value, (-22i64) as u64);

        // gettimeofday(tv, tz): real timeval; tz ignored.
        let mut timeval = GuestTimeval {
            tv_sec: -1,
            tv_usec: -1,
        };
        let timeval_ptr = &mut timeval as *mut GuestTimeval as u64;
        let result = dispatch_linux_x64(
            linux_abi::linux::GETTIMEOFDAY,
            &context([timeval_ptr, 0, 0, 0, 0, 0]),
        );
        assert_eq!(result.value, 0);
        assert_eq!(timeval.tv_sec, 2);
        assert_eq!(timeval.tv_usec, 500_000);

        // Decided rows without an executable path stay ENOSYS: read, close,
        // dup, mmap family, yield.
        for number in [
            linux_abi::linux::READ,
            linux_abi::linux::CLOSE,
            linux_abi::linux::DUP,
            linux_abi::linux::DUP2,
            linux_abi::linux::MMAP,
            linux_abi::linux::MUNMAP,
            linux_abi::linux::MPROTECT,
            linux_abi::linux::SCHED_YIELD,
        ] {
            let result = dispatch_linux_x64(number, &context([0; 6]));
            assert_eq!(result.value, (-38i64) as u64, "linux syscall {number}");
            assert_eq!(result.error, None);
        }

        // Undecided numbers (fork, socket, openat) also stay ENOSYS.
        for number in [2u32, 41, 57, 59, 257] {
            let result = dispatch_linux_x64(number, &context([0; 6]));
            assert_eq!(result.value, (-38i64) as u64, "linux syscall {number}");
        }

        // --- ARM64 twin: same shared helpers, Linux ARM64 numbering. ---
        // write(1, buf, 8) through the ARM64 table (Linux arm64 write=64).
        let result = dispatch_linux_arm64(
            linux_abi::linux_arm64::WRITE,
            &context([1, buffer_ptr, 8, 0, 0, 0]),
        );
        assert_eq!(result.value, 8);
        assert_eq!(result.error, None);
        assert_eq!(WRITE_CALLS.load(Ordering::SeqCst), 3);
        assert_eq!(WRITE_BYTES.load(Ordering::SeqCst), 16);

        // write on an unsupported fd: -EBADF, console untouched.
        let result = dispatch_linux_arm64(
            linux_abi::linux_arm64::WRITE,
            &context([7, buffer_ptr, 4, 0, 0, 0]),
        );
        assert_eq!(result.value, (-9i64) as u64);
        assert_eq!(WRITE_CALLS.load(Ordering::SeqCst), 3);

        // exit / exit_group (93 / 94): status passthrough.
        let result =
            dispatch_linux_arm64(linux_abi::linux_arm64::EXIT, &context([42, 0, 0, 0, 0, 0]));
        assert!(matches!(
            result.action,
            super::super::SyscallAction::ExitCurrentThread { status: 42 }
        ));
        let result = dispatch_linux_arm64(
            linux_abi::linux_arm64::EXIT_GROUP,
            &context([7, 0, 0, 0, 0, 0]),
        );
        assert!(matches!(
            result.action,
            super::super::SyscallAction::ExitCurrentThread { status: 7 }
        ));

        // clock_gettime (113) / gettimeofday (169): same guest layouts as
        // the x86_64 twin.
        let result = dispatch_linux_arm64(
            linux_abi::linux_arm64::CLOCK_GETTIME,
            &context([linux_abi::clock::CLOCK_MONOTONIC, timespec_ptr, 0, 0, 0, 0]),
        );
        assert_eq!(result.value, 0);
        assert_eq!(timespec.tv_sec, 2);
        assert_eq!(timespec.tv_nsec, 500_000_000);

        let result = dispatch_linux_arm64(
            linux_abi::linux_arm64::GETTIMEOFDAY,
            &context([timeval_ptr, 0, 0, 0, 0, 0]),
        );
        assert_eq!(result.value, 0);
        assert_eq!(timeval.tv_sec, 2);
        assert_eq!(timeval.tv_usec, 500_000);

        // Decided ARM64 rows without an executable path stay ENOSYS:
        // read, close, dup, mmap family, yield.
        for number in [
            linux_abi::linux_arm64::READ,
            linux_abi::linux_arm64::CLOSE,
            linux_abi::linux_arm64::DUP,
            linux_abi::linux_arm64::MMAP,
            linux_abi::linux_arm64::MUNMAP,
            linux_abi::linux_arm64::MPROTECT,
            linux_abi::linux_arm64::SCHED_YIELD,
        ] {
            let result = dispatch_linux_arm64(number, &context([0; 6]));
            assert_eq!(
                result.value,
                (-38i64) as u64,
                "linux arm64 syscall {number}"
            );
            assert_eq!(result.error, None);
        }
    }

    #[test]
    fn linux_arm64_dispatch_rejects_outside_its_table() {
        // Undecided ARM64 numbers (io_setup, openat, brk, getpid, execve,
        // socket) stay ENOSYS, and x86_64-only numbers do not leak into
        // the ARM64 twin: its table gate rejects them before the family
        // match (dup2 was removed from the arm64 ABI, so x86 dup2=33 is
        // not an ARM64 row either).
        for number in [0u32, 1, 33, 56, 172, 198, 214, 221] {
            let result = dispatch_linux_arm64(number, &context([0; 6]));
            assert_eq!(
                result.value,
                (-38i64) as u64,
                "linux arm64 syscall {number}"
            );
            assert_eq!(result.error, None);
        }
    }

    #[test]
    fn host_arch_selection_routes_to_the_matching_table() {
        // Pins the compile-time selection on whatever arch runs the test:
        // the twin matching the host arch executes its decided numbers,
        // the other arch's numbers fall outside its table and stay ENOSYS.
        #[cfg(target_arch = "x86_64")]
        {
            let result =
                dispatch_linux_syscall(linux_abi::linux::EXIT, &context([42, 0, 0, 0, 0, 0]));
            assert!(matches!(
                result.action,
                super::super::SyscallAction::ExitCurrentThread { status: 42 }
            ));
            let result = dispatch_linux_syscall(linux_abi::linux_arm64::EXIT, &context([0; 6]));
            assert_eq!(result.value, (-38i64) as u64);
        }
        #[cfg(target_arch = "aarch64")]
        {
            let result =
                dispatch_linux_syscall(linux_abi::linux_arm64::EXIT, &context([42, 0, 0, 0, 0, 0]));
            assert!(matches!(
                result.action,
                super::super::SyscallAction::ExitCurrentThread { status: 42 }
            ));
            let result = dispatch_linux_syscall(linux_abi::linux::EXIT, &context([0; 6]));
            assert_eq!(result.value, (-38i64) as u64);
        }
    }
}
