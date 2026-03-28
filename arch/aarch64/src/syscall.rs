#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyscallBringupStatus {
    pub implemented: bool,
}

pub const fn bringup_status() -> SyscallBringupStatus {
    SyscallBringupStatus { implemented: false }
}
