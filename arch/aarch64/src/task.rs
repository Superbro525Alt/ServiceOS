#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskBringupStatus {
    pub implemented: bool,
}

pub const fn bringup_status() -> TaskBringupStatus {
    TaskBringupStatus { implemented: false }
}
