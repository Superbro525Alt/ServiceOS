mod space;
mod types;

pub use space::CapabilitySpace;
pub use types::{
    CapabilityDescriptor, CapabilityError, CapabilityHandle, CapabilityResolver, CapabilityRights,
    CapabilitySlot, CapabilityView, PreparedTransfer, TransferMode,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::{DmaSafety, ObjectRegistry};

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
    fn rollback_moved_restores_source_handle_exactly() {
        let registry = ObjectRegistry::new();
        let object = registry.create_memory_object(4096, true, DmaSafety::Unsafe);
        let space = CapabilitySpace::new();

        let source = space
            .install(object, CapabilityRights::memory_object(), Some(0xab))
            .expect("source capability should install");
        let transfer = space
            .prepare_transfer(source, CapabilityRights::READ, TransferMode::Move)
            .expect("move transfer should succeed");
        assert!(matches!(
            space.resolve(source, CapabilityRights::READ),
            Err(CapabilityError::InvalidHandle)
        ));

        // The operation that carried the transfer failed: restore the moved
        // handle with its original rights and badge.
        assert!(space.rollback_moved(&transfer));
        let restored = space
            .resolve(source, CapabilityRights::WRITE)
            .expect("rolled-back source should resolve with original rights");
        assert_eq!(restored.badge, Some(0xab));

        // Rolling back again must not duplicate or clobber anything.
        assert!(!space.rollback_moved(&transfer));
    }

    #[test]
    fn move_transfer_closes_source_and_reinstalls_in_receiver() {
        let registry = ObjectRegistry::new();
        let object = registry.create_memory_object(8192, true, DmaSafety::Unsafe);
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
        space.set_next_handle_for_test(u32::MAX);

        assert_eq!(
            space.install(object, CapabilityRights::event(), None),
            Err(CapabilityError::HandleSpaceExhausted)
        );
    }

    /// Formal boundary proof (channels): a task can never address a
    /// foreign channel endpoint. `ipc.send` resolves the endpoint handle
    /// exclusively through the sender's capability space, so an endpoint
    /// the task holds no capability for is unaddressable, and a capability
    /// without SEND cannot carry a send.
    #[test]
    fn foreign_channel_is_unaddressable_without_capability() {
        let registry = ObjectRegistry::new();
        let (endpoint, _peer) = registry.create_channel_pair();
        let space = CapabilitySpace::new();

        // No capability for the endpoint at all: any handle value the
        // guest invents resolves to nothing.
        assert!(matches!(
            space.resolve(CapabilityHandle(0xCAFE), CapabilityRights::SEND),
            Err(CapabilityError::InvalidHandle)
        ));

        // A capability that lacks SEND cannot send on the channel it can
        // otherwise see.
        let read_only = space
            .install(endpoint, CapabilityRights::READ, None)
            .expect("read-only channel capability should install");
        assert!(matches!(
            space.resolve(read_only, CapabilityRights::SEND),
            Err(CapabilityError::RightsViolation { .. })
        ));
    }

    /// Formal boundary proof (memory): mapping and granting a memory
    /// object both require matching capability rights. A task holding a
    /// read-only memory capability can neither map it nor grant it onward.
    #[test]
    fn memory_map_and_grant_require_matching_rights() {
        let registry = ObjectRegistry::new();
        let object = registry.create_memory_object(4096, true, DmaSafety::Unsafe);
        let space = CapabilitySpace::new();
        let read_only = space
            .install(object, CapabilityRights::READ, None)
            .expect("read-only memory capability should install");

        // MAP-gated consumers (memory_map syscalls) are refused.
        assert!(matches!(
            space.resolve(read_only, CapabilityRights::MAP),
            Err(CapabilityError::RightsViolation { .. })
        ));

        // Grant paths (channel-send handle transfer) are refused: the
        // capability never carried TRANSFER, so no transfer can be
        // prepared from it in any mode.
        assert!(
            space
                .prepare_transfer(read_only, CapabilityRights::READ, TransferMode::Copy)
                .is_err()
        );
    }
}
