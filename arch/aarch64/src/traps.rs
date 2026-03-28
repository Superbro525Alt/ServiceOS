#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TrapBringupStatus {
    pub implemented: bool,
}

pub const fn bringup_status() -> TrapBringupStatus {
    TrapBringupStatus { implemented: false }
}
