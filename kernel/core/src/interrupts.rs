#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct InterruptVector(pub u16);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct ExceptionVector(pub u8);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TrapFrameView {
    pub instruction_pointer: u64,
    pub stack_pointer: u64,
    pub flags: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FaultDisposition {
    Fatal,
    Retry,
    DeliverToTask,
}

pub trait InterruptController {
    fn enable_vector(&mut self, vector: InterruptVector);
    fn disable_vector(&mut self, vector: InterruptVector);
}
