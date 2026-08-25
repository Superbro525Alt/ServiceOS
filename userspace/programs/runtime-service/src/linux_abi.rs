//! Linux-oriented ABI stub layer: syscall-number translation.
//!
//! Contract (documented, not yet a hosted implementation): a future hosted
//! posix guest translates Linux x86_64 syscall numbers through
//! [`translate_syscall`] before issuing the ServiceOS `int80`-style gate.
//! Mappings exist only where the existing kernel syscall set has a real
//! equivalent — everything else resolves to `ENOSYS` so guests fail loudly
//! instead of hitting an unintended kernel surface. Pure mapping only: this
//! module performs no syscalls and requires no kernel changes.
//!
//! Host-testable scaffolding: nothing here is wired into a live dispatch
//! path yet, so non-test builds carry it as documented dead code.
#![cfg_attr(not(test), allow(dead_code))]

/// ServiceOS "unsupported" verdict for the translated call. The hosted stub
/// must return `ENOSYS` to the guest rather than issue any kernel call.
pub(crate) const TRANSLATE_ENOSYS: u32 = u32::MAX;

/// Linux x86_64 syscall numbers referenced by the table.
mod linux {
    pub(super) const READ: u32 = 0;
    pub(super) const WRITE: u32 = 1;
    pub(super) const CLOSE: u32 = 3;
    pub(super) const SCHED_YIELD: u32 = 24;
    pub(super) const DUP: u32 = 32;
    pub(super) const DUP2: u32 = 33;
    pub(super) const MMAP: u32 = 9;
    pub(super) const MUNMAP: u32 = 11;
    pub(super) const MPROTECT: u32 = 10;
    pub(super) const GETTIMEOFDAY: u32 = 96;
    pub(super) const CLOCK_GETTIME: u32 = 228;
    pub(super) const EXIT: u32 = 60;
    pub(super) const EXIT_GROUP: u32 = 231;
}

/// ServiceOS kernel syscall numbers (`serviceos_abi::SyscallNumber` values).
///
/// Kept as local constants so the mapping table stays a host-testable pure
/// function independent of the no_std ABI crate's build configuration.
mod sos {
    pub(super) const MONOTONIC_NOW: u32 = 1;
    pub(super) const THREAD_EXIT: u32 = 2;
    pub(super) const YIELD_CURRENT: u32 = 3;
    pub(super) const DEBUG_CONSOLE_READ: u32 = 13;
    pub(super) const DEBUG_CONSOLE_WRITE: u32 = 14;
    pub(super) const HANDLE_DUPLICATE: u32 = 8;
    pub(super) const HANDLE_CLOSE: u32 = 9;
    pub(super) const MEMORY_MAP: u32 = 27;
    pub(super) const MEMORY_UNMAP: u32 = 42;
    pub(super) const MEMORY_PROTECT: u32 = 43;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SyscallMapping {
    /// Linux x86_64 syscall number.
    pub(crate) linux_number: u32,
    /// ServiceOS syscall number, or [`TRANSLATE_ENOSYS`] when the ABI has no
    /// equivalent and the stub must return ENOSYS.
    pub(crate) serviceos_number: u32,
    /// Short rationale for audit/inspection output.
    pub(crate) note: &'static str,
}

const LINUX_READ: SyscallMapping = SyscallMapping {
    linux_number: linux::READ,
    serviceos_number: sos::DEBUG_CONSOLE_READ,
    note: "console-scoped read; fd semantics unsupported",
};
const LINUX_WRITE: SyscallMapping = SyscallMapping {
    linux_number: linux::WRITE,
    serviceos_number: sos::DEBUG_CONSOLE_WRITE,
    note: "console-scoped write; fd semantics unsupported",
};

/// The translation skeleton: one row per Linux number with a decided
/// mapping. Unlisted numbers are implicitly ENOSYS.
const SYSCALL_TABLE: &[SyscallMapping] = &[
    LINUX_READ,
    LINUX_WRITE,
    SyscallMapping {
        linux_number: linux::CLOSE,
        serviceos_number: sos::HANDLE_CLOSE,
        note: "descriptor close",
    },
    SyscallMapping {
        linux_number: linux::MMAP,
        serviceos_number: sos::MEMORY_MAP,
        note: "anonymous/file mappings narrowed at stub layer",
    },
    SyscallMapping {
        linux_number: linux::MPROTECT,
        serviceos_number: sos::MEMORY_PROTECT,
        note: "W^X policy still enforced by kernel",
    },
    SyscallMapping {
        linux_number: linux::MUNMAP,
        serviceos_number: sos::MEMORY_UNMAP,
        note: "address-space unmap",
    },
    SyscallMapping {
        linux_number: linux::SCHED_YIELD,
        serviceos_number: sos::YIELD_CURRENT,
        note: "scheduler yield",
    },
    SyscallMapping {
        linux_number: linux::DUP,
        serviceos_number: sos::HANDLE_DUPLICATE,
        note: "handle duplicate (no fd table yet)",
    },
    SyscallMapping {
        linux_number: linux::DUP2,
        serviceos_number: sos::HANDLE_DUPLICATE,
        note: "handle duplicate (fd renumbering unsupported)",
    },
    SyscallMapping {
        linux_number: linux::EXIT,
        serviceos_number: sos::THREAD_EXIT,
        note: "task exit",
    },
    SyscallMapping {
        linux_number: linux::GETTIMEOFDAY,
        serviceos_number: sos::MONOTONIC_NOW,
        note: "wall-clock semantics approximated by monotonic now",
    },
    SyscallMapping {
        linux_number: linux::CLOCK_GETTIME,
        serviceos_number: sos::MONOTONIC_NOW,
        note: "only CLOCK_MONOTONIC honored",
    },
    SyscallMapping {
        linux_number: linux::EXIT_GROUP,
        serviceos_number: sos::THREAD_EXIT,
        note: "single-threaded guests exit via ThreadExit",
    },
];

/// Translate a Linux x86_64 syscall number into its ServiceOS equivalent.
/// Returns [`TRANSLATE_ENOSYS`] for numbers without a decided mapping —
/// including the whole file/process/socket/signal families that would need
/// IPC round-trips or kernel surfaces that do not exist.
pub(crate) fn translate_syscall(linux_number: u32) -> u32 {
    translate_detail(linux_number)
        .map(|mapping| mapping.serviceos_number)
        .unwrap_or(TRANSLATE_ENOSYS)
}

/// Full mapping row for inspection/audit; `None` means ENOSYS.
pub(crate) fn translate_detail(linux_number: u32) -> Option<&'static SyscallMapping> {
    SYSCALL_TABLE
        .iter()
        .find(|mapping| mapping.linux_number == linux_number)
}

/// Number of decided (non-ENOSYS-by-default family) rows in the table.
pub(crate) fn mapped_syscall_count() -> usize {
    SYSCALL_TABLE.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serviceos_userspace_runtime as rt;

    #[test]
    fn maps_decided_numbers_to_serviceos_equivalents() {
        assert_eq!(translate_syscall(linux::READ), sos::DEBUG_CONSOLE_READ);
        assert_eq!(translate_syscall(linux::WRITE), sos::DEBUG_CONSOLE_WRITE);
        assert_eq!(translate_syscall(linux::CLOSE), sos::HANDLE_CLOSE);
        assert_eq!(translate_syscall(linux::MMAP), sos::MEMORY_MAP);
        assert_eq!(translate_syscall(linux::MPROTECT), sos::MEMORY_PROTECT);
        assert_eq!(translate_syscall(linux::MUNMAP), sos::MEMORY_UNMAP);
        assert_eq!(translate_syscall(linux::SCHED_YIELD), sos::YIELD_CURRENT);
        assert_eq!(translate_syscall(linux::DUP2), sos::HANDLE_DUPLICATE);
        assert_eq!(translate_syscall(linux::GETTIMEOFDAY), sos::MONOTONIC_NOW);
        assert_eq!(translate_syscall(linux::CLOCK_GETTIME), sos::MONOTONIC_NOW);
        assert_eq!(translate_syscall(linux::EXIT), sos::THREAD_EXIT);
        assert_eq!(translate_syscall(linux::EXIT_GROUP), sos::THREAD_EXIT);
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
                x if x == rt::SyscallNumber::MonotonicNow as u32
                    || x == rt::SyscallNumber::ThreadExit as u32
                    || x == rt::SyscallNumber::YieldCurrent as u32
                    || x == rt::SyscallNumber::HandleDuplicate as u32
                    || x == rt::SyscallNumber::HandleClose as u32
                    || x == rt::SyscallNumber::DebugConsoleRead as u32
                    || x == rt::SyscallNumber::DebugConsoleWrite as u32
                    || x == rt::SyscallNumber::MemoryMap as u32
                    || x == rt::SyscallNumber::MemoryUnmap as u32
                    || x == rt::SyscallNumber::MemoryProtect as u32
            );
            assert!(
                known,
                "row {} targets unknown syscall {}",
                mapping.linux_number, mapping.serviceos_number
            );
        }
    }
}
