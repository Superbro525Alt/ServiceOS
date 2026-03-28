use crate::object::{KernelObjectRef, ObjectId};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct CapabilityHandle(pub u32);

pub type CapabilitySlot = CapabilityHandle;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapabilityRights(u64);

impl CapabilityRights {
    pub const NONE: Self = Self(0);
    pub const READ: Self = Self(1 << 0);
    pub const WRITE: Self = Self(1 << 1);
    pub const MAP: Self = Self(1 << 2);
    pub const SIGNAL: Self = Self(1 << 3);
    pub const WAIT: Self = Self(1 << 4);
    pub const SEND: Self = Self(1 << 5);
    pub const RECEIVE: Self = Self(1 << 6);
    pub const DUPLICATE: Self = Self(1 << 7);
    pub const TRANSFER: Self = Self(1 << 8);
    pub const MANAGE: Self = Self(1 << 9);

    pub const fn bits(self) -> u64 {
        self.0
    }

    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub const fn intersect(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    pub const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    pub const fn channel_endpoint() -> Self {
        Self::READ
            .union(Self::WRITE)
            .union(Self::SEND)
            .union(Self::RECEIVE)
            .union(Self::DUPLICATE)
            .union(Self::TRANSFER)
    }

    pub const fn task() -> Self {
        Self::READ
            .union(Self::WRITE)
            .union(Self::MANAGE)
            .union(Self::DUPLICATE)
            .union(Self::TRANSFER)
    }

    pub const fn thread() -> Self {
        Self::READ
            .union(Self::WRITE)
            .union(Self::MANAGE)
            .union(Self::WAIT)
            .union(Self::DUPLICATE)
            .union(Self::TRANSFER)
    }

    pub const fn memory_object() -> Self {
        Self::READ
            .union(Self::WRITE)
            .union(Self::MAP)
            .union(Self::DUPLICATE)
            .union(Self::TRANSFER)
    }

    pub const fn timer() -> Self {
        Self::READ
            .union(Self::WRITE)
            .union(Self::WAIT)
            .union(Self::DUPLICATE)
            .union(Self::TRANSFER)
    }

    pub const fn event() -> Self {
        Self::READ
            .union(Self::SIGNAL)
            .union(Self::WAIT)
            .union(Self::DUPLICATE)
            .union(Self::TRANSFER)
    }

    pub const fn bootstrap() -> Self {
        Self::MANAGE
    }

    pub const fn packet_interface() -> Self {
        Self::READ
            .union(Self::WRITE)
            .union(Self::WAIT)
            .union(Self::DUPLICATE)
            .union(Self::TRANSFER)
    }

    pub const fn display_output() -> Self {
        Self::READ
            .union(Self::WRITE)
            .union(Self::DUPLICATE)
            .union(Self::TRANSFER)
    }

    pub const fn input_source() -> Self {
        Self::READ
            .union(Self::WAIT)
            .union(Self::DUPLICATE)
            .union(Self::TRANSFER)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapabilityDescriptor {
    pub handle: CapabilityHandle,
    pub object: ObjectId,
    pub rights: CapabilityRights,
    pub badge: Option<u64>,
}

#[derive(Clone)]
pub struct CapabilityView {
    pub handle: CapabilityHandle,
    pub object: KernelObjectRef,
    pub rights: CapabilityRights,
    pub badge: Option<u64>,
}

#[derive(Clone)]
pub struct PreparedTransfer {
    pub(super) object: KernelObjectRef,
    pub(super) rights: CapabilityRights,
    pub(super) badge: Option<u64>,
}

impl PreparedTransfer {
    pub fn descriptor(&self, handle: CapabilityHandle) -> CapabilityDescriptor {
        CapabilityDescriptor {
            handle,
            object: self.object.id(),
            rights: self.rights,
            badge: self.badge,
        }
    }

    pub fn object(&self) -> &KernelObjectRef {
        &self.object
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferMode {
    Move,
    Copy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityError {
    InvalidHandle,
    HandleSpaceExhausted,
    RightsViolation {
        required: CapabilityRights,
        actual: CapabilityRights,
    },
    RequestedRightsExceedSource,
    DuplicateForbidden,
    TransferForbidden,
}

pub trait CapabilityResolver {
    fn resolve_descriptor(&self, handle: CapabilityHandle) -> Option<CapabilityDescriptor>;
}
