#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct ObjectId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectKind {
    AddressSpace,
    Task,
    Thread,
    Endpoint,
    Notification,
    Timer,
    VmObject,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObjectHeader {
    pub id: ObjectId,
    pub kind: ObjectKind,
}

pub trait KernelObject {
    fn header(&self) -> ObjectHeader;
}
