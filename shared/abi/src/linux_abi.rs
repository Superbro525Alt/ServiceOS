//! Linux-x86_64 guest syscall-ABI contract: number translation + guest-side
//! error encoding.
//!
//! Contract: a task spawned with `SPAWN_ABI_LINUX_SYSCALL` (carried in the
//! additive `TaskSpawnImage` argument slot) enters the kernel through the
//! same `int 0x80` gate as native tasks, but the dispatcher first translates
//! the Linux x86_64 syscall number in `rax` through [`translate_syscall`].
//!
//! Guest-visible result encoding for `linux-syscall` tasks (the Linux
//! convention, replacing the native `rax = value / rdx = error` split):
//! - success: `rax = syscall value` (byte counts, 0, ...), `rdx = 0`
//! - failure: `rax = -(errno)` (two's complement), `rdx = 0`
//! - no equivalent kernel call: `rax = -ENOSYS`
//!
//! Only families with a real ServiceOS equivalent have decided mappings;
//! everything else — including table rows the kernel does not yet execute —
//! resolves `-ENOSYS` so guests fail loudly instead of reaching an
//! unintended kernel surface. One table exists per guest CPU architecture:
//! [`linux`] numbers the x86_64 table (number issued in `rax` through the
//! `int 0x80` gate) and [`linux_arm64`] the ARM64 table (number issued in
//! `w8` through `svc #0`). The kernel selects the table by its compile-time
//! architecture; other architectures have no guest ABI yet.
//!
//! Pure mapping/encoding only: no syscalls, no kernel dependencies.

/// Internal "no decided mapping" verdict (never a valid Linux number).
pub const TRANSLATE_ENOSYS: u32 = u32::MAX;

/// `TaskSpawnImage` ABI-flag word values (additive argument slot).
pub mod spawn_abi {
    /// Native ServiceOS syscall numbering (the default; flag word 0).
    pub const NATIVE: u64 = 0;
    /// Task enters through Linux x86_64 number translation.
    pub const LINUX_SYSCALL: u64 = 0x534f_534c_494e_5558; // "SOSLINUX"
}

/// Linux errno values used by the guest ABI.
pub mod errno {
    pub const EPERM: u64 = 1;
    pub const ENOENT: u64 = 2;
    pub const EBADF: u64 = 9;
    pub const EAGAIN: u64 = 11;
    pub const ENOMEM: u64 = 12;
    pub const EFAULT: u64 = 14;
    pub const EBUSY: u64 = 16;
    pub const EINTR: u64 = 4;
    pub const EINVAL: u64 = 22;
    pub const EPIPE: u64 = 32;
    pub const ENOSYS: u64 = 38;
}

/// Linux x86_64 syscall numbers referenced by the x86_64 table.
pub mod linux {
    pub const READ: u32 = 0;
    pub const WRITE: u32 = 1;
    pub const CLOSE: u32 = 3;
    pub const SCHED_YIELD: u32 = 24;
    pub const DUP: u32 = 32;
    pub const DUP2: u32 = 33;
    pub const MMAP: u32 = 9;
    pub const MUNMAP: u32 = 11;
    pub const MPROTECT: u32 = 10;
    pub const GETTIMEOFDAY: u32 = 96;
    pub const CLOCK_GETTIME: u32 = 228;
    pub const EXIT: u32 = 60;
    pub const EXIT_GROUP: u32 = 231;
}

/// Linux `CLOCK_*` ids honored by the clock family.
pub mod clock {
    pub const CLOCK_MONOTONIC: u64 = 1;
}

/// Linux ARM64 (aarch64, LP64) syscall numbers referenced by the ARM64
/// table. The generic arm64 kernel numbering; `dup2` does not exist here
/// (it was removed from the arm64 ABI, `dup3` = 24 stays undecided), so
/// the handle family maps `dup` only.
pub mod linux_arm64 {
    pub const DUP: u32 = 23;
    pub const MPROTECT: u32 = 28;
    pub const CLOSE: u32 = 57;
    pub const READ: u32 = 63;
    pub const WRITE: u32 = 64;
    pub const EXIT: u32 = 93;
    pub const EXIT_GROUP: u32 = 94;
    pub const CLOCK_GETTIME: u32 = 113;
    pub const SCHED_YIELD: u32 = 124;
    pub const GETTIMEOFDAY: u32 = 169;
    pub const MUNMAP: u32 = 215;
    pub const MMAP: u32 = 222;
}

/// Guest-visible `struct timespec` (Linux LP64 layout: identical on the
/// x86_64 and aarch64 guest ABIs).
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GuestTimespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

/// Guest-visible `struct timeval` (Linux LP64 layout: identical on the
/// x86_64 and aarch64 guest ABIs).
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GuestTimeval {
    pub tv_sec: i64,
    pub tv_usec: i64,
}

/// One decided mapping row: Linux number → ServiceOS syscall number.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyscallMapping {
    /// Linux syscall number (per the table's guest architecture).
    pub linux_number: u32,
    /// ServiceOS syscall number, or [`TRANSLATE_ENOSYS`] when the ABI has no
    /// equivalent and the guest must see `-ENOSYS`.
    pub serviceos_number: u32,
    /// Short rationale for audit/inspection output.
    pub note: &'static str,
}

const LINUX_READ: SyscallMapping = SyscallMapping {
    linux_number: linux::READ,
    serviceos_number: 13, // DebugConsoleRead
    note: "console-scoped read; fd semantics unsupported",
};
const LINUX_WRITE: SyscallMapping = SyscallMapping {
    linux_number: linux::WRITE,
    serviceos_number: 14, // DebugConsoleWrite
    note: "console-scoped write; fd semantics unsupported",
};

/// The translation table: one row per Linux number with a decided mapping.
/// Unlisted numbers are implicitly ENOSYS.
const SYSCALL_TABLE: &[SyscallMapping] = &[
    LINUX_READ,
    LINUX_WRITE,
    SyscallMapping {
        linux_number: linux::CLOSE,
        serviceos_number: 9, // HandleClose
        note: "descriptor close",
    },
    SyscallMapping {
        linux_number: linux::MMAP,
        serviceos_number: 27, // MemoryMap
        note: "anonymous/file mappings narrowed at stub layer",
    },
    SyscallMapping {
        linux_number: linux::MPROTECT,
        serviceos_number: 43, // MemoryProtect
        note: "W^X policy still enforced by kernel",
    },
    SyscallMapping {
        linux_number: linux::MUNMAP,
        serviceos_number: 42, // MemoryUnmap
        note: "address-space unmap",
    },
    SyscallMapping {
        linux_number: linux::SCHED_YIELD,
        serviceos_number: 3, // YieldCurrent
        note: "scheduler yield",
    },
    SyscallMapping {
        linux_number: linux::DUP,
        serviceos_number: 8, // HandleDuplicate
        note: "handle duplicate (no fd table yet)",
    },
    SyscallMapping {
        linux_number: linux::DUP2,
        serviceos_number: 8, // HandleDuplicate
        note: "handle duplicate (fd renumbering unsupported)",
    },
    SyscallMapping {
        linux_number: linux::EXIT,
        serviceos_number: 2, // ThreadExit
        note: "task exit",
    },
    SyscallMapping {
        linux_number: linux::GETTIMEOFDAY,
        serviceos_number: 1, // MonotonicNow
        note: "wall-clock semantics approximated by monotonic now",
    },
    SyscallMapping {
        linux_number: linux::CLOCK_GETTIME,
        serviceos_number: 1, // MonotonicNow
        note: "only CLOCK_MONOTONIC honored",
    },
    SyscallMapping {
        linux_number: linux::EXIT_GROUP,
        serviceos_number: 2, // ThreadExit
        note: "single-threaded guests exit via ThreadExit",
    },
];

/// Translate a Linux x86_64 syscall number into its ServiceOS equivalent.
/// Returns [`TRANSLATE_ENOSYS`] for numbers without a decided mapping —
/// including the whole file/process/socket/signal families that would need
/// IPC round-trips or kernel surfaces that do not exist.
pub fn translate_syscall(linux_number: u32) -> u32 {
    translate_detail(linux_number)
        .map(|mapping| mapping.serviceos_number)
        .unwrap_or(TRANSLATE_ENOSYS)
}

/// Full mapping row for inspection/audit; `None` means ENOSYS.
pub fn translate_detail(linux_number: u32) -> Option<&'static SyscallMapping> {
    SYSCALL_TABLE
        .iter()
        .find(|mapping| mapping.linux_number == linux_number)
}

/// Number of decided rows in the table.
pub fn mapped_syscall_count() -> usize {
    SYSCALL_TABLE.len()
}

/// Encode a failure for a `linux-syscall` guest: `rax = -(errno)`.
pub const fn error_result(errno: u64) -> u64 {
    (errno as i64).wrapping_neg() as u64
}

/// Map a ServiceOS `SyscallErrorCode` (as returned in the native `rdx`
/// error slot) onto the Linux errno a `linux-syscall` guest should observe.
/// Unmapped codes fall back to `ENOSYS` by contract.
pub const fn errno_for_serviceos_error(code: u64) -> u64 {
    match code {
        1 => errno::ENOSYS,  // Unsupported
        2 => errno::ENOSYS,  // InvalidCall
        3 => errno::EPERM,   // PermissionDenied
        4 => errno::ENOSYS,  // NotInitialized
        5 => errno::EINVAL,  // InvalidArgument
        6 => errno::EINVAL,  // BufferTooSmall
        7 => errno::EAGAIN,  // QueueEmpty
        8 => errno::ENOENT,  // NotFound
        9 => errno::EBUSY,   // Busy
        10 => errno::ENOMEM, // CapacityExceeded
        11 => errno::EPIPE,  // BrokenPipe
        _ => errno::ENOSYS,  // Ok and anything unknown
    }
}

/// Convert a monotonic tick count at `tick_hz` into guest timespec fields.
/// `tick_hz == 0` collapses to zero (the kernel rejects such clocks at
/// init); saturation keeps absurd tick counts from wrapping.
pub const fn ticks_to_timespec(ticks: u64, tick_hz: u64) -> (i64, i64) {
    if tick_hz == 0 {
        return (0, 0);
    }
    let nanos_per_tick = 1_000_000_000u64 / tick_hz;
    let total_nanos = ticks.saturating_mul(nanos_per_tick);
    let sec = (total_nanos / 1_000_000_000) as i64;
    let nsec = (total_nanos % 1_000_000_000) as i64;
    (sec, nsec)
}

/// Convert a monotonic tick count at `tick_hz` into guest timeval fields.
pub const fn ticks_to_timeval(ticks: u64, tick_hz: u64) -> (i64, i64) {
    if tick_hz == 0 {
        return (0, 0);
    }
    let micros_per_tick = 1_000_000u64 / tick_hz;
    let total_micros = ticks.saturating_mul(micros_per_tick);
    let sec = (total_micros / 1_000_000) as i64;
    let usec = (total_micros % 1_000_000) as i64;
    (sec, usec)
}

const LINUX_ARM64_DUP: SyscallMapping = SyscallMapping {
    linux_number: linux_arm64::DUP,
    serviceos_number: 8, // HandleDuplicate
    note: "handle duplicate (no fd table yet)",
};

/// The ARM64 translation table: one row per Linux ARM64 number with a
/// decided mapping. Same family coverage as the x86_64 table (console /
/// handle / memory / clock / exit), keyed by the Linux ARM64 numbering;
/// unlisted numbers are implicitly ENOSYS.
const SYSCALL_TABLE_ARM64: &[SyscallMapping] = &[
    LINUX_ARM64_DUP,
    SyscallMapping {
        linux_number: linux_arm64::MPROTECT,
        serviceos_number: 43, // MemoryProtect
        note: "W^X policy still enforced by kernel",
    },
    SyscallMapping {
        linux_number: linux_arm64::CLOSE,
        serviceos_number: 9, // HandleClose
        note: "descriptor close",
    },
    SyscallMapping {
        linux_number: linux_arm64::READ,
        serviceos_number: 13, // DebugConsoleRead
        note: "console-scoped read; fd semantics unsupported",
    },
    SyscallMapping {
        linux_number: linux_arm64::WRITE,
        serviceos_number: 14, // DebugConsoleWrite
        note: "console-scoped write; fd semantics unsupported",
    },
    SyscallMapping {
        linux_number: linux_arm64::EXIT,
        serviceos_number: 2, // ThreadExit
        note: "task exit",
    },
    SyscallMapping {
        linux_number: linux_arm64::EXIT_GROUP,
        serviceos_number: 2, // ThreadExit
        note: "single-threaded guests exit via ThreadExit",
    },
    SyscallMapping {
        linux_number: linux_arm64::CLOCK_GETTIME,
        serviceos_number: 1, // MonotonicNow
        note: "only CLOCK_MONOTONIC honored",
    },
    SyscallMapping {
        linux_number: linux_arm64::SCHED_YIELD,
        serviceos_number: 3, // YieldCurrent
        note: "scheduler yield",
    },
    SyscallMapping {
        linux_number: linux_arm64::GETTIMEOFDAY,
        serviceos_number: 1, // MonotonicNow
        note: "wall-clock semantics approximated by monotonic now",
    },
    SyscallMapping {
        linux_number: linux_arm64::MUNMAP,
        serviceos_number: 42, // MemoryUnmap
        note: "address-space unmap",
    },
    SyscallMapping {
        linux_number: linux_arm64::MMAP,
        serviceos_number: 27, // MemoryMap
        note: "anonymous/file mappings narrowed at stub layer",
    },
];

/// Translate a Linux ARM64 syscall number into its ServiceOS equivalent.
/// Returns [`TRANSLATE_ENOSYS`] for numbers without a decided mapping —
/// the same contract as [`translate_syscall`], keyed by the ARM64
/// numbering.
pub fn translate_arm64_syscall(linux_number: u32) -> u32 {
    translate_arm64_detail(linux_number)
        .map(|mapping| mapping.serviceos_number)
        .unwrap_or(TRANSLATE_ENOSYS)
}

/// Full ARM64 mapping row for inspection/audit; `None` means ENOSYS.
pub fn translate_arm64_detail(linux_number: u32) -> Option<&'static SyscallMapping> {
    SYSCALL_TABLE_ARM64
        .iter()
        .find(|mapping| mapping.linux_number == linux_number)
}

/// Number of decided rows in the ARM64 table.
pub fn mapped_arm64_syscall_count() -> usize {
    SYSCALL_TABLE_ARM64.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SyscallNumber;

    #[test]
    fn maps_decided_numbers_to_serviceos_equivalents() {
        assert_eq!(
            translate_syscall(linux::READ),
            SyscallNumber::DebugConsoleRead as u32
        );
        assert_eq!(
            translate_syscall(linux::WRITE),
            SyscallNumber::DebugConsoleWrite as u32
        );
        assert_eq!(
            translate_syscall(linux::CLOSE),
            SyscallNumber::HandleClose as u32
        );
        assert_eq!(
            translate_syscall(linux::MMAP),
            SyscallNumber::MemoryMap as u32
        );
        assert_eq!(
            translate_syscall(linux::MPROTECT),
            SyscallNumber::MemoryProtect as u32
        );
        assert_eq!(
            translate_syscall(linux::MUNMAP),
            SyscallNumber::MemoryUnmap as u32
        );
        assert_eq!(
            translate_syscall(linux::SCHED_YIELD),
            SyscallNumber::YieldCurrent as u32
        );
        assert_eq!(
            translate_syscall(linux::DUP),
            SyscallNumber::HandleDuplicate as u32
        );
        assert_eq!(
            translate_syscall(linux::DUP2),
            SyscallNumber::HandleDuplicate as u32
        );
        assert_eq!(
            translate_syscall(linux::GETTIMEOFDAY),
            SyscallNumber::MonotonicNow as u32
        );
        assert_eq!(
            translate_syscall(linux::CLOCK_GETTIME),
            SyscallNumber::MonotonicNow as u32
        );
        assert_eq!(
            translate_syscall(linux::EXIT),
            SyscallNumber::ThreadExit as u32
        );
        assert_eq!(
            translate_syscall(linux::EXIT_GROUP),
            SyscallNumber::ThreadExit as u32
        );
    }

    #[test]
    fn unmapped_families_resolve_enosys() {
        // Filesystem family: open/openat/stat/fstat/lseek have no storage
        // syscall — storage flows through IPC services instead.
        assert_eq!(translate_syscall(2), TRANSLATE_ENOSYS); // open
        assert_eq!(translate_syscall(5), TRANSLATE_ENOSYS); // fstat
        assert_eq!(translate_syscall(8), TRANSLATE_ENOSYS); // lseek
        assert_eq!(translate_syscall(257), TRANSLATE_ENOSYS); // openat
        // Process family: fork/clone/execve route through manager IPC.
        assert_eq!(translate_syscall(57), TRANSLATE_ENOSYS); // fork
        assert_eq!(translate_syscall(56), TRANSLATE_ENOSYS); // clone
        assert_eq!(translate_syscall(59), TRANSLATE_ENOSYS); // execve
        // Socket and signal families: no kernel surface.
        assert_eq!(translate_syscall(41), TRANSLATE_ENOSYS); // socket
        assert_eq!(translate_syscall(42), TRANSLATE_ENOSYS); // connect
        assert_eq!(translate_syscall(13), TRANSLATE_ENOSYS); // rt_sigaction
        assert_eq!(translate_syscall(62), TRANSLATE_ENOSYS); // kill
    }

    #[test]
    fn table_rows_are_unique_and_sorted_by_linux_number() {
        for (left, right) in SYSCALL_TABLE.iter().zip(SYSCALL_TABLE.iter().skip(1)) {
            assert!(left.linux_number < right.linux_number);
        }
    }

    #[test]
    fn every_mapped_target_exists_in_serviceos_abi() {
        for mapping in SYSCALL_TABLE {
            let known = matches!(
                mapping.serviceos_number,
                x if x == SyscallNumber::MonotonicNow as u32
                    || x == SyscallNumber::ThreadExit as u32
                    || x == SyscallNumber::YieldCurrent as u32
                    || x == SyscallNumber::HandleDuplicate as u32
                    || x == SyscallNumber::HandleClose as u32
                    || x == SyscallNumber::DebugConsoleRead as u32
                    || x == SyscallNumber::DebugConsoleWrite as u32
                    || x == SyscallNumber::MemoryMap as u32
                    || x == SyscallNumber::MemoryUnmap as u32
                    || x == SyscallNumber::MemoryProtect as u32
            );
            assert!(
                known,
                "row {} targets unknown syscall {}",
                mapping.linux_number, mapping.serviceos_number
            );
        }
    }

    #[test]
    fn table_keeps_linux_number_collision_distinct_from_native_numbering() {
        // The hazard this module exists to solve: Linux and ServiceOS numbers
        // collide on the shared int 0x80 gate (Linux write=1 is ServiceOS
        // MonotonicNow, Linux mmap=9 is ServiceOS HandleClose, Linux
        // exit=60 is an unallocated ServiceOS slot).
        assert_ne!(linux::WRITE, SyscallNumber::DebugConsoleWrite as u32);
        assert_eq!(translate_syscall(60), SyscallNumber::ThreadExit as u32);
        assert_ne!(60, SyscallNumber::ChannelSend as u32);
    }

    #[test]
    fn spawn_abi_flag_is_not_a_small_accidental_value() {
        assert_ne!(spawn_abi::NATIVE, spawn_abi::LINUX_SYSCALL);
        assert!(spawn_abi::LINUX_SYSCALL > u32::MAX as u64);
    }

    #[test]
    fn error_results_are_negative_linux_errnos() {
        assert_eq!(error_result(errno::ENOSYS), (-38i64) as u64);
        assert_eq!(error_result(errno::EINVAL), (-22i64) as u64);
    }

    #[test]
    fn serviceos_errors_map_onto_linux_errnos() {
        assert_eq!(errno_for_serviceos_error(0), errno::ENOSYS); // Ok is not an error input
        assert_eq!(errno_for_serviceos_error(1), errno::ENOSYS); // Unsupported
        assert_eq!(errno_for_serviceos_error(2), errno::ENOSYS); // InvalidCall
        assert_eq!(errno_for_serviceos_error(3), errno::EPERM); // PermissionDenied
        assert_eq!(errno_for_serviceos_error(4), errno::ENOSYS); // NotInitialized
        assert_eq!(errno_for_serviceos_error(5), errno::EINVAL); // InvalidArgument
        assert_eq!(errno_for_serviceos_error(6), errno::EINVAL); // BufferTooSmall
        assert_eq!(errno_for_serviceos_error(7), errno::EAGAIN); // QueueEmpty
        assert_eq!(errno_for_serviceos_error(8), errno::ENOENT); // NotFound
        assert_eq!(errno_for_serviceos_error(9), errno::EBUSY); // Busy
        assert_eq!(errno_for_serviceos_error(10), errno::ENOMEM); // CapacityExceeded
        assert_eq!(errno_for_serviceos_error(11), errno::EPIPE); // BrokenPipe
        assert_eq!(errno_for_serviceos_error(99), errno::ENOSYS); // unknown
    }

    #[test]
    fn tick_conversions_match_tick_rate() {
        // 1500 ticks at 1000 Hz = 1.5 s.
        assert_eq!(ticks_to_timespec(1500, 1000), (1, 500_000_000));
        assert_eq!(ticks_to_timeval(1500, 1000), (1, 500_000));
        // Zero ticks and zero rate stay zero.
        assert_eq!(ticks_to_timespec(0, 1000), (0, 0));
        assert_eq!(ticks_to_timespec(10, 0), (0, 0));
        // Sub-tick remainder truncates toward zero.
        assert_eq!(ticks_to_timespec(1, 2), (0, 500_000_000));
    }

    #[test]
    fn guest_time_layouts_match_linux_lp64_guests() {
        assert_eq!(core::mem::size_of::<GuestTimespec>(), 16);
        assert_eq!(core::mem::size_of::<GuestTimeval>(), 16);
    }

    #[test]
    fn arm64_maps_decided_numbers_to_serviceos_equivalents() {
        assert_eq!(
            translate_arm64_syscall(linux_arm64::READ),
            SyscallNumber::DebugConsoleRead as u32
        );
        assert_eq!(
            translate_arm64_syscall(linux_arm64::WRITE),
            SyscallNumber::DebugConsoleWrite as u32
        );
        assert_eq!(
            translate_arm64_syscall(linux_arm64::CLOSE),
            SyscallNumber::HandleClose as u32
        );
        assert_eq!(
            translate_arm64_syscall(linux_arm64::MMAP),
            SyscallNumber::MemoryMap as u32
        );
        assert_eq!(
            translate_arm64_syscall(linux_arm64::MPROTECT),
            SyscallNumber::MemoryProtect as u32
        );
        assert_eq!(
            translate_arm64_syscall(linux_arm64::MUNMAP),
            SyscallNumber::MemoryUnmap as u32
        );
        assert_eq!(
            translate_arm64_syscall(linux_arm64::SCHED_YIELD),
            SyscallNumber::YieldCurrent as u32
        );
        assert_eq!(
            translate_arm64_syscall(linux_arm64::DUP),
            SyscallNumber::HandleDuplicate as u32
        );
        assert_eq!(
            translate_arm64_syscall(linux_arm64::GETTIMEOFDAY),
            SyscallNumber::MonotonicNow as u32
        );
        assert_eq!(
            translate_arm64_syscall(linux_arm64::CLOCK_GETTIME),
            SyscallNumber::MonotonicNow as u32
        );
        assert_eq!(
            translate_arm64_syscall(linux_arm64::EXIT),
            SyscallNumber::ThreadExit as u32
        );
        assert_eq!(
            translate_arm64_syscall(linux_arm64::EXIT_GROUP),
            SyscallNumber::ThreadExit as u32
        );
    }

    #[test]
    fn arm64_unmapped_families_resolve_enosys() {
        // Filesystem family: openat/stat/fstat/lseek have no storage
        // syscall — storage flows through IPC services instead.
        assert_eq!(translate_arm64_syscall(56), TRANSLATE_ENOSYS); // openat
        assert_eq!(translate_arm64_syscall(79), TRANSLATE_ENOSYS); // fstatat
        assert_eq!(translate_arm64_syscall(62), TRANSLATE_ENOSYS); // lseek
        // Process family: clone/execve route through manager IPC; getpid
        // has no kernel surface.
        assert_eq!(translate_arm64_syscall(220), TRANSLATE_ENOSYS); // clone
        assert_eq!(translate_arm64_syscall(221), TRANSLATE_ENOSYS); // execve
        assert_eq!(translate_arm64_syscall(172), TRANSLATE_ENOSYS); // getpid
        // Memory family rows that stay undecided (brk, mremap).
        assert_eq!(translate_arm64_syscall(214), TRANSLATE_ENOSYS); // brk
        assert_eq!(translate_arm64_syscall(216), TRANSLATE_ENOSYS); // mremap
        // Socket, signal, and I/O families: no kernel surface.
        assert_eq!(translate_arm64_syscall(198), TRANSLATE_ENOSYS); // socket
        assert_eq!(translate_arm64_syscall(134), TRANSLATE_ENOSYS); // rt_sigaction
        assert_eq!(translate_arm64_syscall(129), TRANSLATE_ENOSYS); // kill
        assert_eq!(translate_arm64_syscall(0), TRANSLATE_ENOSYS); // io_setup
        // ARM64-specific gaps: dup2 was removed from the arm64 ABI and
        // dup3 stays undecided.
        assert_eq!(translate_arm64_syscall(33), TRANSLATE_ENOSYS); // x86 dup2
        assert_eq!(translate_arm64_syscall(24), TRANSLATE_ENOSYS); // dup3
        // x86_64-only numbers must not leak into the ARM64 table.
        assert_eq!(translate_arm64_syscall(1), TRANSLATE_ENOSYS); // x86 write
        assert_eq!(translate_arm64_syscall(60), TRANSLATE_ENOSYS); // x86 exit
    }

    #[test]
    fn arm64_table_rows_are_unique_and_sorted_by_linux_number() {
        for (left, right) in SYSCALL_TABLE_ARM64
            .iter()
            .zip(SYSCALL_TABLE_ARM64.iter().skip(1))
        {
            assert!(left.linux_number < right.linux_number);
        }
    }

    #[test]
    fn arm64_every_mapped_target_exists_in_serviceos_abi() {
        for mapping in SYSCALL_TABLE_ARM64 {
            let known = matches!(
                mapping.serviceos_number,
                x if x == SyscallNumber::MonotonicNow as u32
                    || x == SyscallNumber::ThreadExit as u32
                    || x == SyscallNumber::YieldCurrent as u32
                    || x == SyscallNumber::HandleDuplicate as u32
                    || x == SyscallNumber::HandleClose as u32
                    || x == SyscallNumber::DebugConsoleRead as u32
                    || x == SyscallNumber::DebugConsoleWrite as u32
                    || x == SyscallNumber::MemoryMap as u32
                    || x == SyscallNumber::MemoryUnmap as u32
                    || x == SyscallNumber::MemoryProtect as u32
            );
            assert!(
                known,
                "row {} targets unknown syscall {}",
                mapping.linux_number, mapping.serviceos_number
            );
        }
    }

    #[test]
    fn arm64_and_x86_64_tables_are_disjoint_and_family_aligned() {
        // A number decided in one table must never carry a different
        // meaning (or silently reuse a row) in the other.
        for left in SYSCALL_TABLE {
            for right in SYSCALL_TABLE_ARM64 {
                assert_ne!(
                    left.linux_number, right.linux_number,
                    "linux number {} decided in both tables",
                    left.linux_number
                );
            }
        }
        // Same families, same ServiceOS targets: per-table row counts may
        // differ (arm64 has no dup2), but every x86_64 target number must
        // also appear in the ARM64 table and vice versa.
        fn x86_rows() -> [u32; 32] {
            let mut rows = [0u32; 32];
            for (index, mapping) in SYSCALL_TABLE.iter().enumerate() {
                rows[index] = mapping.serviceos_number;
            }
            rows
        }
        fn arm64_rows() -> [u32; 32] {
            let mut rows = [0u32; 32];
            for (index, mapping) in SYSCALL_TABLE_ARM64.iter().enumerate() {
                rows[index] = mapping.serviceos_number;
            }
            rows
        }
        fn sorted_unique(source: &[u32]) -> ([u32; 32], usize) {
            let mut out = [0u32; 32];
            let mut len = 0;
            for value in source {
                if !out[..len].contains(value) {
                    out[len] = *value;
                    len += 1;
                }
            }
            out[..len].sort_unstable();
            (out, len)
        }
        let (x86_values, x86_len) = sorted_unique(&x86_rows());
        let (arm64_values, arm64_len) = sorted_unique(&arm64_rows());
        assert_eq!(x86_len, arm64_len);
        assert_eq!(x86_values[..x86_len], arm64_values[..arm64_len]);
        assert_eq!(mapped_arm64_syscall_count(), 12);
        assert_eq!(mapped_syscall_count(), 13);
    }
}
