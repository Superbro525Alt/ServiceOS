use alloc::{collections::BTreeMap, sync::Arc, vec::Vec};
use spin::Mutex;

use super::{
    CapabilityDescriptor, CapabilityError, CapabilityHandle, CapabilityResolver, CapabilityRights,
    CapabilityView, PreparedTransfer, TransferMode,
};
use crate::object::KernelObjectRef;

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

    #[cfg(test)]
    pub fn set_next_handle_for_test(&self, next_handle: u32) {
        self.state.lock().next_handle = next_handle;
    }
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
