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
        space.set_next_handle_for_test(u32::MAX);

        assert_eq!(
            space.install(object, CapabilityRights::event(), None),
            Err(CapabilityError::HandleSpaceExhausted)
        );
    }
}
