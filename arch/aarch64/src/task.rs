#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskBringupStatus {
    pub context_switch: bool,
}

pub const fn bringup_status() -> TaskBringupStatus {
    TaskBringupStatus {
        context_switch: false,
    }
}
