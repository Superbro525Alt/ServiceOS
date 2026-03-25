#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct AddressSpaceId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct TaskId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct ThreadId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionState {
    Constructing,
    Runnable,
    Running,
    Blocked,
    Suspended,
    Dying,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulingContext {
    pub budget_ticks: u64,
    pub period_ticks: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskCreationError {
    UnsupportedInPhase0,
    ResourceUnavailable,
}

pub trait TaskManager {
    fn root_task(&self) -> Option<TaskId>;
    fn create_address_space(&mut self) -> Result<AddressSpaceId, TaskCreationError>;
}
