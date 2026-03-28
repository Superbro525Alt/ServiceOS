#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MmuBringupStatus {
    pub implemented: bool,
}

pub const fn bringup_status() -> MmuBringupStatus {
    MmuBringupStatus { implemented: false }
}
