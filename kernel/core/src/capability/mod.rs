use crate::object::ObjectId;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct CapabilitySlot(pub u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapabilityRights(u64);

impl CapabilityRights {
    pub const READ: Self = Self(1 << 0);
    pub const WRITE: Self = Self(1 << 1);
    pub const MAP: Self = Self(1 << 2);
    pub const SIGNAL: Self = Self(1 << 3);
    pub const TRANSFER: Self = Self(1 << 4);

    pub const fn bits(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapabilityDescriptor {
    pub object: ObjectId,
    pub rights: CapabilityRights,
    pub badge: Option<u64>,
}

pub trait CapabilitySpace {
    fn resolve(&self, slot: CapabilitySlot) -> Option<CapabilityDescriptor>;
}
