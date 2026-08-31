//! Linux guest syscall-ABI support: re-exports of the shared-ABI
//! translation table plus the ServiceOS number cross-checks the runtime
//! service relies on. The kernel is the live dispatch authority; this
//! module only decides and surfaces the per-environment mode.
//!
//! `serviceos_userspace_runtime` glob-re-exports the shared ABI crate, so
//! the table is reachable both as `rt::linux_abi` and through the direct
//! dependency in crates that have one.

pub(crate) use serviceos_userspace_runtime::linux_abi::{
    TRANSLATE_ENOSYS, mapped_syscall_count, translate_detail, translate_syscall,
};

#[cfg(test)]
mod tests {
    use super::{TRANSLATE_ENOSYS, mapped_syscall_count, translate_detail, translate_syscall};
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
    }

    #[test]
    fn table_and_enosys_marker_survive_the_reexport() {
        assert_eq!(mapped_syscall_count(), 13);
        assert_eq!(translate_syscall(59), TRANSLATE_ENOSYS);
        assert!(translate_detail(rt::linux_abi::linux::EXIT).is_some());
    }
}
