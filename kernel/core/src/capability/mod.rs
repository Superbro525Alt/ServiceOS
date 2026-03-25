use alloc::{collections::BTreeMap, sync::Arc, vec::Vec};
use spin::Mutex;

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

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub const fn intersect(self, other: Self) -> Self {
        Self(self.0 & other.0)
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
    object: KernelObjectRef,
    rights: CapabilityRights,
    badge: Option<u64>,
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferMode {
    Move,
    Copy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityError {
    InvalidHandle,
    RightsViolation {
        required: CapabilityRights,
        actual: CapabilityRights,
    },
    RequestedRightsExceedSource,
    DuplicateForbidden,
    TransferForbidden,
}

#[derive(Clone)]
struct CapabilityEntry {
    object: KernelObjectRef,
    rights: CapabilityRights,
    badge: Option<u64>,
}

struct CapabilitySpaceState {
    next_handle: u32,
    entries: BTreeMap<CapabilityHandle, CapabilityEntry>,
}

pub struct CapabilitySpace {
    state: Mutex<CapabilitySpaceState>,
}

impl CapabilitySpace {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(CapabilitySpaceState {
                next_handle: 1,
                entries: BTreeMap::new(),
            }),
        }
    }

    pub fn install(
        &self,
        object: KernelObjectRef,
        rights: CapabilityRights,
        badge: Option<u64>,
    ) -> CapabilityHandle {
        let mut state = self.state.lock();
        let handle = CapabilityHandle(state.next_handle);
        state.next_handle = state.next_handle.saturating_add(1);
        state.entries.insert(
            handle,
            CapabilityEntry {
                object,
                rights,
                badge,
            },
        );
        handle
    }

    pub fn resolve(
        &self,
        handle: CapabilityHandle,
        required: CapabilityRights,
    ) -> Result<CapabilityView, CapabilityError> {
        let state = self.state.lock();
        let Some(entry) = state.entries.get(&handle) else {
            return Err(CapabilityError::InvalidHandle);
        };

        if !entry.rights.contains(required) {
            return Err(CapabilityError::RightsViolation {
                required,
                actual: entry.rights,
            });
        }

        Ok(CapabilityView {
            handle,
            object: Arc::clone(&entry.object),
            rights: entry.rights,
            badge: entry.badge,
        })
    }

    pub fn duplicate(
        &self,
        source: CapabilityHandle,
        requested: CapabilityRights,
        badge_override: Option<Option<u64>>,
    ) -> Result<CapabilityHandle, CapabilityError> {
        let mut state = self.state.lock();
        let Some(entry) = state.entries.get(&source).cloned() else {
            return Err(CapabilityError::InvalidHandle);
        };

        if !entry.rights.contains(CapabilityRights::DUPLICATE) {
            return Err(CapabilityError::DuplicateForbidden);
        }

        if !entry.rights.contains(requested) {
            return Err(CapabilityError::RequestedRightsExceedSource);
        }

        let handle = CapabilityHandle(state.next_handle);
        state.next_handle = state.next_handle.saturating_add(1);
        state.entries.insert(
            handle,
            CapabilityEntry {
                object: entry.object,
                rights: requested,
                badge: badge_override.unwrap_or(entry.badge),
            },
        );
        Ok(handle)
    }

    pub fn prepare_transfer(
        &self,
        handle: CapabilityHandle,
        requested: CapabilityRights,
        mode: TransferMode,
    ) -> Result<PreparedTransfer, CapabilityError> {
        let mut state = self.state.lock();
        let Some(entry) = state.entries.get(&handle).cloned() else {
            return Err(CapabilityError::InvalidHandle);
        };

        if !entry.rights.contains(CapabilityRights::TRANSFER) {
            return Err(CapabilityError::TransferForbidden);
        }

        if mode == TransferMode::Copy && !entry.rights.contains(CapabilityRights::DUPLICATE) {
            return Err(CapabilityError::DuplicateForbidden);
        }

        if !entry.rights.contains(requested) {
            return Err(CapabilityError::RequestedRightsExceedSource);
        }

        if mode == TransferMode::Move {
            state.entries.remove(&handle);
        }

        Ok(PreparedTransfer {
            object: entry.object,
            rights: requested,
            badge: entry.badge,
        })
    }

    pub fn accept_transfer(&self, transfer: PreparedTransfer) -> CapabilityHandle {
        self.install(transfer.object, transfer.rights, transfer.badge)
    }

    pub fn close(&self, handle: CapabilityHandle) -> Result<CapabilityDescriptor, CapabilityError> {
        let mut state = self.state.lock();
        let Some(entry) = state.entries.remove(&handle) else {
            return Err(CapabilityError::InvalidHandle);
        };

        Ok(CapabilityDescriptor {
            handle,
            object: entry.object.id(),
            rights: entry.rights,
            badge: entry.badge,
        })
    }

    pub fn handle_count(&self) -> usize {
        self.state.lock().entries.len()
    }

    pub fn list(&self) -> Vec<CapabilityDescriptor> {
        let state = self.state.lock();

        state
            .entries
            .iter()
            .map(|(handle, entry)| CapabilityDescriptor {
                handle: *handle,
                object: entry.object.id(),
                rights: entry.rights,
                badge: entry.badge,
            })
            .collect()
    }
}

pub trait CapabilityResolver {
    fn resolve_descriptor(&self, handle: CapabilityHandle) -> Option<CapabilityDescriptor>;
}

impl CapabilityResolver for CapabilitySpace {
    fn resolve_descriptor(&self, handle: CapabilityHandle) -> Option<CapabilityDescriptor> {
        self.state
            .lock()
            .entries
            .get(&handle)
            .map(|entry| CapabilityDescriptor {
                handle,
                object: entry.object.id(),
                rights: entry.rights,
                badge: entry.badge,
            })
    }
}
