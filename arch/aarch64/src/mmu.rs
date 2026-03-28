#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MmuBringupStatus {
    pub page_tables: bool,
}

pub const fn bringup_status() -> MmuBringupStatus {
    MmuBringupStatus { page_tables: false }
}
