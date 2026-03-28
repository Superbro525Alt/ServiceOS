#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyscallBringupStatus {
    pub entry_path: bool,
}

pub const fn bringup_status() -> SyscallBringupStatus {
    SyscallBringupStatus { entry_path: false }
}
