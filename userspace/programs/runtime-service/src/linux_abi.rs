//! Linux guest syscall-ABI support: re-exports of the shared-ABI
//! translation tables plus the ServiceOS number cross-checks the runtime
//! service relies on. The kernel is the live dispatch authority; this
//! module only decides and surfaces the per-environment mode. Two Linux
//! tables exist, one per guest CPU architecture (x86_64, ARM64); the
//! kernel selects by its compile-time architecture, so the runtime only
//! reports which table the build's kernel-side mapping carries.
//!
//! `serviceos_userspace_runtime` glob-re-exports the shared ABI crate, so
//! the tables are reachable both as `rt::linux_abi` and through the direct
//! dependency in crates that have one.

// Test-only names ride along with the runtime-visible re-exports so the
// module stays the single ABI surface for the service.
#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use serviceos_userspace_runtime::linux_abi::{
    TRANSLATE_ENOSYS, mapped_arm64_syscall_count, mapped_syscall_count, translate_arm64_detail,
    translate_arm64_syscall, translate_detail, translate_syscall,
};

/// Guest syscall table rows available for this build's architecture:
/// the x86_64 table on x86_64 builds, the ARM64 table on aarch64 builds,
/// 0 where the kernel has no guest ABI (riscv64). Surfaced additively as
/// env-status word 13.
pub(crate) fn guest_table_rows_for_build() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        mapped_syscall_count() as u64
    }
    #[cfg(target_arch = "aarch64")]
    {
        mapped_arm64_syscall_count() as u64
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TRANSLATE_ENOSYS, guest_table_rows_for_build, mapped_arm64_syscall_count,
        mapped_syscall_count, translate_arm64_detail, translate_arm64_syscall, translate_detail,
        translate_syscall,
    };
    use serviceos_userspace_runtime as rt;

    #[test]
    fn shared_table_targets_match_the_runtime_view_of_the_abi() {
        // The runtime service gates guest launches on this family set; the
        // shared table must keep targeting numbers the runtime crate sees.
        assert_eq!(
            translate_syscall(rt::linux_abi::linux::EXIT),
            rt::SyscallNumber::ThreadExit as u32
        );
        assert_eq!(
            translate_syscall(rt::linux_abi::linux::WRITE),
            rt::SyscallNumber::DebugConsoleWrite as u32
        );
        assert_eq!(
            translate_syscall(rt::linux_abi::linux::CLOCK_GETTIME),
            rt::SyscallNumber::MonotonicNow as u32
        );
        assert_eq!(
            translate_syscall(rt::linux_abi::linux::EXIT_GROUP),
            rt::SyscallNumber::ThreadExit as u32
        );
        // The ARM64 table covers the same families at the ARM64 numbers.
        assert_eq!(
            translate_arm64_syscall(rt::linux_abi::linux_arm64::EXIT),
            rt::SyscallNumber::ThreadExit as u32
        );
        assert_eq!(
            translate_arm64_syscall(rt::linux_abi::linux_arm64::WRITE),
            rt::SyscallNumber::DebugConsoleWrite as u32
        );
        assert_eq!(
            translate_arm64_syscall(rt::linux_abi::linux_arm64::CLOCK_GETTIME),
            rt::SyscallNumber::MonotonicNow as u32
        );
        assert_eq!(
            translate_arm64_syscall(rt::linux_abi::linux_arm64::EXIT_GROUP),
            rt::SyscallNumber::ThreadExit as u32
        );
    }

    #[test]
    fn tables_and_enosys_marker_survive_the_reexport() {
        assert_eq!(mapped_syscall_count(), 13);
        assert_eq!(mapped_arm64_syscall_count(), 12);
        assert_eq!(translate_syscall(59), TRANSLATE_ENOSYS);
        assert_eq!(translate_arm64_syscall(221), TRANSLATE_ENOSYS); // execve
        assert!(translate_detail(rt::linux_abi::linux::EXIT).is_some());
        assert!(translate_arm64_detail(rt::linux_abi::linux_arm64::EXIT).is_some());
    }

    #[test]
    fn guest_table_rows_report_this_builds_arch() {
        // Host tests run on the build host; the cfg mirrors the env-status
        // surface. Other arches report 0 (no guest table).
        #[cfg(target_arch = "x86_64")]
        assert_eq!(guest_table_rows_for_build(), 13);
        #[cfg(target_arch = "aarch64")]
        assert_eq!(guest_table_rows_for_build(), 12);
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        assert_eq!(guest_table_rows_for_build(), 0);
    }
}
