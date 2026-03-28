#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TrapBringupStatus {
    pub vector_table: bool,
}

pub const fn bringup_status() -> TrapBringupStatus {
    TrapBringupStatus {
        vector_table: false,
    }
}
