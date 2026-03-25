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
    ) -> Result<CapabilityHandle, CapabilityError> {
        let mut state = self.state.lock();
        let handle = allocate_handle(&mut state)?;
        state.entries.insert(
            handle,
            CapabilityEntry {
                object,
                rights,
                badge,
            },
        );
        Ok(handle)
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

        let handle = allocate_handle(&mut state)?;
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

    pub fn accept_transfer(
        &self,
        transfer: PreparedTransfer,
    ) -> Result<CapabilityHandle, CapabilityError> {
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

fn allocate_handle(state: &mut CapabilitySpaceState) -> Result<CapabilityHandle, CapabilityError> {
    let handle = CapabilityHandle(state.next_handle);
    state.next_handle = state
        .next_handle
        .checked_add(1)
        .ok_or(CapabilityError::HandleSpaceExhausted)?;
    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::ObjectRegistry;

    #[test]
    fn duplicate_restricts_rights_and_preserves_badge() {
        let registry = ObjectRegistry::new();
        let object = registry.create_event(false);
        let space = CapabilitySpace::new();

        let source = space
            .install(object, CapabilityRights::event(), Some(0x55aa))
            .expect("source capability should install");
        let duplicate = space
            .duplicate(
                source,
                CapabilityRights::READ.union(CapabilityRights::WAIT),
                None,
            )
            .expect("duplicate should succeed");

        let duplicate_view = space
            .resolve(duplicate, CapabilityRights::WAIT)
            .expect("duplicate should carry requested rights");
        assert_eq!(
            duplicate_view.rights,
            CapabilityRights::READ.union(CapabilityRights::WAIT)
        );
        assert_eq!(duplicate_view.badge, Some(0x55aa));
    }

    #[test]
    fn move_transfer_closes_source_and_reinstalls_in_receiver() {
        let registry = ObjectRegistry::new();
        let object = registry.create_memory_object(8192, true);
        let sender = CapabilitySpace::new();
        let receiver = CapabilitySpace::new();

        let source = sender
            .install(object, CapabilityRights::memory_object(), Some(7))
            .expect("source capability should install");
        let transfer = sender
            .prepare_transfer(
                source,
                CapabilityRights::READ.union(CapabilityRights::MAP),
                TransferMode::Move,
            )
            .expect("move transfer should succeed");

        assert!(matches!(
            sender.resolve(source, CapabilityRights::READ),
            Err(CapabilityError::InvalidHandle)
        ));

        let received = receiver
            .accept_transfer(transfer)
            .expect("receiver should accept transfer");
        let received_view = receiver
            .resolve(received, CapabilityRights::MAP)
            .expect("received capability should resolve");
        assert_eq!(
            received_view.rights,
            CapabilityRights::READ.union(CapabilityRights::MAP)
        );
        assert_eq!(received_view.badge, Some(7));
    }

    #[test]
    fn install_reports_handle_exhaustion() {
        let registry = ObjectRegistry::new();
        let object = registry.create_event(false);
        let space = CapabilitySpace::new();
        space.state.lock().next_handle = u32::MAX;

        assert_eq!(
            space.install(object, CapabilityRights::event(), None),
            Err(CapabilityError::HandleSpaceExhausted)
        );
    }
}
