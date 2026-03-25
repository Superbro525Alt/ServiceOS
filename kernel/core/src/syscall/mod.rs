#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyscallNumber(pub u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyscallContext {
    pub instruction_pointer: u64,
    pub stack_pointer: u64,
    pub arguments: [u64; 6],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyscallReturn {
    pub value: u64,
    pub error: Option<SyscallError>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyscallError {
    UnsupportedInPhase0,
    InvalidCall,
    PermissionDenied,
}

pub trait SyscallDispatcher {
    fn dispatch(&self, number: SyscallNumber, context: &SyscallContext) -> SyscallReturn;
}
