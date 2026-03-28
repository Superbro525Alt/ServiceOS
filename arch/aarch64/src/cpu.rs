#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CpuBringupStatus {
    pub implemented: bool,
}

pub const fn bringup_status() -> CpuBringupStatus {
    CpuBringupStatus { implemented: false }
}
